// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

use crate::decode::{decode, ScopeType};
use crate::frame::{Frame, ProgramKey};
use crate::import::{HostFunction, ImportResolver, ImportedFunction};
use crate::instance::{LinkError, TableInstance, WASM_PAGE_SIZE};
use crate::memory::Memory;
use crate::memory::SliceMemory;
use crate::module::Module;
use crate::module::{Global, ReferenceType};
use crate::op::{MemArg, Op};
use crate::stack::Stack;
use crate::{FuncType, Instance, TypeSignature, ValueType};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::rc::Rc;

/// How many ticks we allow before we stop execution when running expressions during the link
/// phase (Active data expressions etc)
const EXPR_TICK_LIMIT: usize = 1 << 10;
/// Default tick limit for normal program execution.
const EXEC_TICK_LIMIT: usize = 1 << 24;
/// Upper bound for retained reusable frames to avoid unbounded memory growth.
const FRAME_POOL_LIMIT: usize = 16384;

pub enum Continuation {
    Call(u32),
    /// Call to function in another module
    CallExternal {
        module_id: u32,
        func_id: u32,
        expected_type: Option<(u32, u32)>, // (source_module_id, expected_type_idx)
    },
    /// Call to host function
    CallHost(HostFunction),
    /// Program ran out of instructions
    ProgramEnd,
    /// An explicit return instruction was encountered.
    /// Stack should contain the return value.
    DoneReturn,
}

impl Debug for Continuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Continuation::Call(idx) => write!(f, "Call({idx})"),
            Continuation::CallExternal {
                module_id, func_id, ..
            } => {
                write!(
                    f,
                    "CallExternal {{ module_id: {module_id}, func_id: {func_id} }}"
                )
            }
            Continuation::CallHost(_) => write!(f, "CallHost(<host_function>)"),
            Continuation::ProgramEnd => write!(f, "ProgramEnd"),
            Continuation::DoneReturn => write!(f, "DoneReturn"),
        }
    }
}

#[derive(Debug)]
pub enum Fault {
    /// Ran out of execution ticks
    OutOfTicks,
    /// Result of an expression etc was an unexpected continuation
    UnexpectedResult(Box<Continuation>),
    /// Value stack underflow
    StackUnderflow,
    /// Control stack underflow
    ControlStackUnderflow,
    /// Local variable index out of bounds
    LocalIndexOutOfBounds,
    /// Global variable index out of bounds
    GlobalIndexOutOfBounds,
    /// Memory access out of bounds
    MemoryOutOfBounds,
    /// Memory growth not supported for this memory type, or memory is at maximum size
    CannotGrowMemory,
    /// Unresolvable type index
    UnresolvableTypeIndex(u32),
    /// Invalid reference type
    InvalidRefType,
    /// Null reference dereference
    NullReference,
    /// Integer division by zero
    IntegerDivisionByZero,
    /// Integer overflow
    IntegerOverflow,
    /// Undefined element
    UndefinedElement,
    /// Uninitialized element
    UninitializedElement,
    /// Invalid conversion to integer
    InvalidConversion,
    /// Indirect call type mismatch
    IndirectCallTypeMismatch,
    /// Unreachable instruction executed
    Unreachable,
}

impl Display for Fault {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::OutOfTicks => write!(f, "Out of ticks"),
            Fault::UnexpectedResult(c) => write!(f, "Unexpected result: {c:?}"),
            Fault::StackUnderflow => write!(f, "Stack underflow"),
            Fault::ControlStackUnderflow => write!(f, "Control stack underflow"),
            Fault::LocalIndexOutOfBounds => write!(f, "Local index out of bounds"),
            Fault::GlobalIndexOutOfBounds => write!(f, "Global index out of bounds"),
            Fault::MemoryOutOfBounds => write!(f, "Memory out of bounds"),
            Fault::CannotGrowMemory => write!(f, "Cannot grow memory"),
            Fault::UnresolvableTypeIndex(idx) => write!(f, "Unresolvable type index: {idx}"),
            Fault::InvalidRefType => write!(f, "Invalid reference type"),
            Fault::NullReference => write!(f, "Null reference dereference"),
            Fault::IntegerDivisionByZero => write!(f, "integer divide by zero"),
            Fault::IntegerOverflow => write!(f, "integer overflow"),
            Fault::UndefinedElement => write!(f, "undefined element"),
            Fault::UninitializedElement => write!(f, "uninitialized element"),
            Fault::InvalidConversion => write!(f, "invalid conversion to integer"),
            Fault::IndirectCallTypeMismatch => write!(f, "indirect call type mismatch"),
            Fault::Unreachable => write!(f, "unreachable"),
        }
    }
}

impl Error for Fault {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncRef {
    pub module_id: u32,
    pub func_idx: u32,
}

thread_local! {
    static DETACHED_INSTANCES: RefCell<HashMap<u32, Instance>> = RefCell::new(HashMap::new());
}

pub(crate) fn register_detached_instance(instance: Instance) {
    let owner_id = instance.owner_id;
    DETACHED_INSTANCES.with(|detached| {
        detached.borrow_mut().insert(owner_id, instance);
    });
}

fn take_detached_instance(owner_id: u32) -> Option<Instance> {
    DETACHED_INSTANCES.with(|detached| detached.borrow_mut().remove(&owner_id))
}

/// Helper function for WASM float-to-signed-int truncation
/// WASM spec: i32.trunc_f32_s traps if value is NaN, ±∞, or outside [-2^31, 2^31)
fn trunc_f32_to_i32(value: f32) -> Result<i32, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 reference implementation
    // i32.trunc_f32_s: RMIN = -2147483904.0f, RMAX = 2147483648.0f
    if value <= -2147483904.0f32 || value >= 2147483648.0f32 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as i32)
}

/// Helper function for WASM float-to-unsigned-int truncation
/// WASM spec: i32.trunc_f32_u traps if value is NaN, ±∞, or outside [0, 2^32)
fn trunc_f32_to_u32(value: f32) -> Result<u32, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i32.trunc_f32_u
    if value <= -1.0f32 || value >= 4294967296.0f32 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as u32)
}

/// WASM spec: i32.trunc_f64_s traps if value is NaN, ±∞, or outside [-2^31, 2^31)
fn trunc_f64_to_i32(value: f64) -> Result<i32, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i32.trunc_f64_s
    if value <= -2147483649.0 || value >= 2147483648.0 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as i32)
}

/// WASM spec: i32.trunc_f64_u traps if value is NaN, ±∞, or outside [0, 2^32)
fn trunc_f64_to_u32(value: f64) -> Result<u32, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i32.trunc_f64_u
    if value <= -1.0 || value >= 4294967296.0 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as u32)
}

/// WASM spec: i64.trunc_f32_s traps if value is NaN, ±∞, or outside [-2^63, 2^63)
fn trunc_f32_to_i64(value: f32) -> Result<i64, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i64.trunc_f32_s
    if value <= -9223373136366403584.0f32 || value >= 9223372036854775808.0f32 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as i64)
}

/// WASM spec: i64.trunc_f32_u traps if value is NaN, ±∞, or outside [0, 2^64)
fn trunc_f32_to_u64(value: f32) -> Result<u64, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i64.trunc_f32_u
    if value <= -1.0f32 || value >= 18446744073709551616.0f32 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as u64)
}

/// WASM spec: i64.trunc_f64_s traps if value is NaN, ±∞, or outside [-2^63, 2^63)
fn trunc_f64_to_i64(value: f64) -> Result<i64, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i64.trunc_f64_s
    if value <= -9223372036854777856.0 || value >= 9223372036854775808.0 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as i64)
}

/// WASM spec: i64.trunc_f64_u traps if value is NaN, ±∞, or outside [0, 2^64)
fn trunc_f64_to_u64(value: f64) -> Result<u64, Fault> {
    if value.is_nan() {
        return Err(Fault::InvalidConversion);
    }
    if value.is_infinite() {
        return Err(Fault::IntegerOverflow);
    }
    // WASM spec: Use exact bounds from wasm3 - i64.trunc_f64_u
    if value <= -1.0 || value >= 18446744073709551616.0 {
        return Err(Fault::IntegerOverflow);
    }
    Ok(value.trunc() as u64)
}

fn execute_branch(frame: &mut Frame, types: &[FuncType], depth: usize) -> Result<(), Fault> {
    // All control scopes are valid branch targets. Depth is counted from innermost scope.
    let target_idx = frame
        .control_stack
        .len()
        .checked_sub(1 + depth)
        .ok_or(Fault::ControlStackUnderflow)?;

    let control = &frame.control_stack[target_idx];
    let target_scope_type = control.scope_type;
    let target_stack_width = control.stack_width;
    let target_start_pc = control.start_pc;
    let target_end_pc = control.end_pc;
    let target_signature = control.signature;

    let pop_depth = match target_scope_type {
        ScopeType::Loop => frame.control_stack.len() - 1 - target_idx, // Don't pop the loop
        _ => frame.control_stack.len() - target_idx, // Pop the target block/function too
    };

    // Pop the branch values from the stack (these will be provided to the target label).
    // For loop labels, branch arity is the loop parameter arity. For all other labels,
    // branch arity is the block result arity.
    frame.scratch_values_raw.clear();
    match (target_signature, target_scope_type) {
        (TypeSignature::ValueType(_), ScopeType::Loop) => {
            // Value-typed loop blocks have no explicit params in MVP encoding.
        }
        (TypeSignature::Index(idx), ScopeType::Loop) => {
            debug_assert!((idx as usize) < types.len());
            let ft = unsafe { types.get_unchecked(idx as usize) };
            frame.scratch_values_raw.resize(ft.params.len(), 0);
            for (i, vt) in ft.params.iter().enumerate().rev() {
                debug_assert_ne!(*vt, ValueType::V128);
                frame.scratch_values_raw[i] = frame.stack.pop_u64()?;
            }
        }
        (TypeSignature::ValueType(vt), _) => {
            if vt != ValueType::Unit && frame.stack.width() > 0 {
                debug_assert_ne!(vt, ValueType::V128);
                frame.scratch_values_raw.push(frame.stack.pop_u64()?);
            }
        }
        (TypeSignature::Index(idx), _) => {
            debug_assert!((idx as usize) < types.len());
            let ft = unsafe { types.get_unchecked(idx as usize) };
            frame.scratch_values_raw.resize(ft.results.len(), 0);
            for (i, vt) in ft.results.iter().enumerate().rev() {
                debug_assert_ne!(*vt, ValueType::V128);
                frame.scratch_values_raw[i] = frame.stack.pop_u64()?;
            }
        }
    }

    // Pop all the control blocks up to (but not including) the target for loops;
    // for non-loops this also pops the target.
    let new_control_len = frame
        .control_stack
        .len()
        .checked_sub(pop_depth)
        .ok_or(Fault::ControlStackUnderflow)?;
    frame.control_stack.truncate(new_control_len);

    // Shrink stack to the target block's width
    frame.stack.shrink_to(target_stack_width);

    // Now provide the branch values to the target block
    match (&target_signature, target_scope_type) {
        (TypeSignature::ValueType(vt), _) => {
            if vt != &ValueType::Unit && !frame.scratch_values_raw.is_empty() {
                debug_assert_ne!(*vt, ValueType::V128);
                frame.stack.push_u64(frame.scratch_values_raw[0]);
            }
        }
        (TypeSignature::Index(idx), ScopeType::Loop) => {
            let ft = &types[*idx as usize];
            for (i, vt) in ft.params.iter().enumerate() {
                debug_assert_ne!(*vt, ValueType::V128);
                frame.stack.push_u64(frame.scratch_values_raw[i]);
            }
        }
        (TypeSignature::Index(idx), _) => {
            let ft = &types[*idx as usize];
            for (i, vt) in ft.results.iter().enumerate() {
                debug_assert_ne!(*vt, ValueType::V128);
                frame.stack.push_u64(frame.scratch_values_raw[i]);
            }
        }
    }

    // For structured control flow, we need to find where to jump based on scope type
    match target_scope_type {
        ScopeType::Function => {
            // Branching to function scope means returning from the function
            frame.pc = frame.program().ops.len(); // This will cause the main loop to exit
        }
        ScopeType::Loop => {
            // Branching to loop label jumps to loop body (immediately after StartScope).
            frame.pc = target_start_pc.saturating_add(1);
        }
        _ => {
            // For block/if labels, jump to instruction after matching EndScope.
            frame.pc = target_end_pc.saturating_add(1);
        }
    }

    Ok(())
}

fn pop_table_value(stack: &mut Stack, ref_type: ReferenceType) -> Result<Value, Fault> {
    match ref_type {
        ReferenceType::FuncRef => {
            let ref_val = stack.pop_ref()?;
            Ok(Value::FuncRef(ref_val.map(decode_func_ref)))
        }
        ReferenceType::ExternRef => {
            let ref_val = stack.pop_ref()?;
            Ok(Value::ExternRef(ref_val.map(|v| v as u32)))
        }
    }
}

#[inline(always)]
fn table_elements_unchecked(table: &TableInstance) -> &Vec<Option<Value>> {
    // Fast immutable read path for table lookups in the interpreter hot loop.
    // Execution is single-threaded; mutable table ops are not interleaved with this borrow.
    unsafe { &*table.elements.as_ptr() }
}

fn validate_range(start: u32, len: u32, total_len: usize) -> Result<(usize, usize), Fault> {
    let start = start as usize;
    let len = len as usize;
    let end = start.checked_add(len).ok_or(Fault::MemoryOutOfBounds)?;
    if end > total_len {
        return Err(Fault::MemoryOutOfBounds);
    }
    Ok((start, end))
}

fn execute<const TRACK_TICKS: bool>(
    frame: &mut Frame,
    memories: &mut [&mut dyn Memory],
    globals: &mut [GlobalVar],
    tables: &mut [TableInstance],
    element_segments: &mut [Option<Vec<Value>>],
    data_segments: &mut [Option<Vec<u8>>],
    max_ticks: usize,
    types: &[FuncType],
    functions: &[usize],
    current_module_id: u32,
    current_module_owner_id: u32,
) -> Result<(Continuation, usize), Fault> {
    let mut ticks_used = 0;

    // Fast path: cache memory 0's data slice to avoid dynamic dispatch and bounds checks on hot path
    // We update this cache when MemoryGrow happens.
    // Use raw pointer to get around borrow checker, we only access it via `unsafe { &*memory0_data_ptr }`
    // when we know memory is valid.
    let mut memory0_data_ptr: *mut [u8] = if !memories.is_empty() {
        memories[0].data_mut()
    } else {
        &mut []
    };

    // Cache locals pointer to avoid bounds checking overhead
    let locals_ptr = frame.locals.as_mut_ptr();

    // Cache program ops slice to avoid repeated dereferences and length checks
    let program = unsafe { &*frame.program_ptr() };
    let ops = program.ops.as_slice();
    let ops_len = ops.len();

    let mut pc = frame.pc;

    macro_rules! return_ok {
        ($val:expr) => {{
            frame.pc = pc;
            return Ok($val);
        }};
    }

    macro_rules! return_err {
        ($val:expr) => {{
            frame.pc = pc;
            return Err($val);
        }};
    }

    macro_rules! exec_branch {
        ($depth:expr) => {{
            frame.pc = pc;
            execute_branch(frame, types, $depth as usize)?;
            pc = frame.pc;
        }};
    }

    loop {
        if pc >= ops_len {
            return_ok!((Continuation::ProgramEnd, ticks_used));
        }
        if TRACK_TICKS {
            ticks_used += 1;
            if ticks_used >= max_ticks {
                return_err!(Fault::OutOfTicks);
            }
        }
        let op_ref = unsafe { ops.get_unchecked(pc) };
        let prev_pc = pc;
        pc += 1;

        // Hot-path dispatch without cloning.
        match op_ref {
            Op::Nop => continue,
            Op::Unreachable => return_err!(Fault::Unreachable),
            Op::StartScope(sig, scope_type) => {
                if let TypeSignature::Index(idx) = *sig {
                    if idx as usize >= types.len() {
                        return Err(Fault::UnresolvableTypeIndex(idx));
                    }
                }
                let end_pc = program.scope_end_jumps[prev_pc];
                if end_pc == usize::MAX {
                    return_err!(Fault::Unreachable);
                }
                frame.push_control(*sig, *scope_type, prev_pc, end_pc);
                continue;
            }
            Op::EndScope(c) => {
                // If this is EndScope(Program), we need to preserve the stack for return value.
                if matches!(c, ScopeType::Program) {
                    return_ok!((Continuation::DoneReturn, ticks_used));
                }
                let end_scope = frame.pop_control(types)?;

                // Shrink-stack to the width declared in the control scope.
                frame.stack.shrink_to(end_scope.stack_width);
                match &end_scope.signature {
                    TypeSignature::ValueType(vt) => {
                        if vt != &ValueType::Unit && !frame.scratch_values_raw.is_empty() {
                            debug_assert_ne!(*vt, ValueType::V128);
                            frame.stack.push_u64(frame.scratch_values_raw[0]);
                        }
                    }
                    TypeSignature::Index(idx) => {
                        let ft = &types[*idx as usize];
                        for (i, vt) in ft.results.iter().enumerate() {
                            debug_assert_ne!(*vt, ValueType::V128);
                            frame.stack.push_u64(frame.scratch_values_raw[i]);
                        }
                    }
                }
                continue;
            }
            Op::If => {
                let condition = frame.stack.pop_u32()?;
                if condition == 0 {
                    let jump_target = program.if_else_jumps[prev_pc];
                    if jump_target == usize::MAX {
                        return_err!(Fault::Unreachable);
                    }
                    pc = jump_target;
                }
                continue;
            }
            Op::Else => {
                let jump_target = program.if_else_jumps[prev_pc];
                if jump_target == usize::MAX {
                    return_err!(Fault::Unreachable);
                }
                pc = jump_target;
                continue;
            }
            Op::Br(depth) => {
                exec_branch!(*depth);
                continue;
            }
            Op::BrIf(depth) => {
                let condition = frame.stack.pop_u32()?;
                if condition != 0 {
                    exec_branch!(*depth);
                    continue;
                }
                continue;
            }
            Op::BrIfLocalI64Eqz(local_idx, depth) => {
                debug_assert_eq!(program.local_types[*local_idx as usize], ValueType::I64);
                let v = unsafe { *locals_ptr.add(*local_idx as usize) };
                if v == 0 {
                    exec_branch!(*depth);
                }
                continue;
            }
            Op::BrIfLocalI32GeU(lhs_local, rhs_local, depth) => {
                debug_assert_eq!(program.local_types[*lhs_local as usize], ValueType::I32);
                debug_assert_eq!(program.local_types[*rhs_local as usize], ValueType::I32);
                let lhs = frame.locals[*lhs_local as usize] as u32;
                let rhs = frame.locals[*rhs_local as usize] as u32;
                if lhs >= rhs {
                    exec_branch!(*depth);
                }
                continue;
            }
            Op::BrTable(idx) => {
                let (table, default) = &program.br_tables[*idx as usize];
                let index = frame.stack.pop_u32()? as usize;
                let depth = if index < table.len() {
                    table[index]
                } else {
                    *default
                } as usize;
                exec_branch!(depth);
                continue;
            }
            Op::Return => return_ok!((Continuation::DoneReturn, ticks_used)),
            Op::Call(c) => return_ok!((Continuation::Call(*c), ticks_used)),
            Op::CallIndirect(_type_idx, table_idx) => {
                let expected_type_idx = *_type_idx;
                let table_idx = *table_idx;
                // Pop the table index from the stack (the actual index to use)
                let table_index = frame.stack.pop_u32()?;
                let call_site_pc = prev_pc;

                // Look up the function reference in the specified table
                if table_idx as usize >= tables.len() {
                    return Err(Fault::UndefinedElement);
                }
                let table = &tables[table_idx as usize];
                let table_elements = table_elements_unchecked(table);

                if table_index as usize >= table_elements.len() {
                    return Err(Fault::UndefinedElement);
                }
                let table_generation = table.generation();

                if let Some((cached_target_module_id, cached_target_func_idx)) = frame
                    .lookup_call_indirect_cache(
                        call_site_pc,
                        table_idx,
                        table_index,
                        expected_type_idx,
                        table_generation,
                    )
                {
                    if cached_target_module_id == current_module_id
                        || cached_target_module_id == current_module_owner_id
                    {
                        return_ok!((Continuation::Call(cached_target_func_idx), ticks_used));
                    }
                    return_ok!((
                        Continuation::CallExternal {
                            module_id: cached_target_module_id,
                            func_id: cached_target_func_idx,
                            expected_type: Some((current_module_id, expected_type_idx)),
                        },
                        ticks_used,
                    ));
                }

                match &table_elements[table_index as usize] {
                    None => return_err!(Fault::UninitializedElement),
                    Some(Value::FuncRef(Some(func_ref))) => {
                        if expected_type_idx as usize >= types.len() {
                            return_err!(Fault::UnresolvableTypeIndex(expected_type_idx));
                        }
                        let expected_type = &types[expected_type_idx as usize];

                        let target_module_id = func_ref.module_id;
                        if target_module_id == current_module_id
                            || target_module_id == current_module_owner_id
                        {
                            if (func_ref.func_idx as usize) >= functions.len() {
                                return_err!(Fault::UnresolvableTypeIndex(func_ref.func_idx));
                            }
                            let actual_type_idx = functions[func_ref.func_idx as usize];
                            if actual_type_idx >= types.len() {
                                return_err!(Fault::UnresolvableTypeIndex(actual_type_idx as u32));
                            }
                            if actual_type_idx != expected_type_idx as usize {
                                let actual_type = &types[actual_type_idx];
                                if expected_type.params != actual_type.params
                                    || expected_type.results != actual_type.results
                                {
                                    return_err!(Fault::IndirectCallTypeMismatch);
                                }
                            }
                            frame.update_call_indirect_cache(
                                call_site_pc,
                                table_idx,
                                table_index,
                                expected_type_idx,
                                table_generation,
                                func_ref.module_id,
                                func_ref.func_idx,
                            );
                            return_ok!((Continuation::Call(func_ref.func_idx), ticks_used));
                        } else {
                            frame.update_call_indirect_cache(
                                call_site_pc,
                                table_idx,
                                table_index,
                                expected_type_idx,
                                table_generation,
                                func_ref.module_id,
                                func_ref.func_idx,
                            );
                            return_ok!((
                                Continuation::CallExternal {
                                    module_id: target_module_id,
                                    func_id: func_ref.func_idx,
                                    expected_type: Some((current_module_id, expected_type_idx)),
                                },
                                ticks_used,
                            ));
                        }
                    }
                    Some(Value::FuncRef(None)) => return_err!(Fault::UninitializedElement),
                    _ => return_err!(Fault::UndefinedElement),
                };
            }
            Op::GetLocal(idx) => {
                let val = unsafe { *locals_ptr.add(*idx as usize) };
                let ty = program.local_types[*idx as usize];
                Value::push_raw_to(ty, val, &mut frame.stack);
                continue;
            }
            Op::SetLocal(idx) => {
                let ty = program.local_types[*idx as usize];
                let val = Value::pop_raw_from(ty, &mut frame.stack)?;
                unsafe {
                    *locals_ptr.add(*idx as usize) = val;
                }
                continue;
            }
            Op::TeeLocal(idx) => {
                let ty = program.local_types[*idx as usize];
                let val = Value::top_raw_of(ty, &mut frame.stack)?;
                unsafe {
                    *locals_ptr.add(*idx as usize) = val;
                }
                continue;
            }
            Op::I32LocalMulAddConst(src, mul, add) => {
                debug_assert_eq!(program.local_types[*src as usize], ValueType::I32);
                let v = frame.locals[*src as usize] as i32;
                frame
                    .stack
                    .push_i32(v.wrapping_mul(*mul).wrapping_add(*add));
                continue;
            }
            Op::I32LocalMulAddConstSet(src, mul, add, dst) => {
                debug_assert_eq!(program.local_types[*src as usize], ValueType::I32);
                debug_assert_eq!(program.local_types[*dst as usize], ValueType::I32);
                let v = frame.locals[*src as usize] as i32;
                unsafe {
                    *locals_ptr.add(*dst as usize) =
                        v.wrapping_mul(*mul).wrapping_add(*add) as i64 as u64;
                }
                continue;
            }
            Op::I32LocalAddConstSet(src, add, dst) => {
                debug_assert_eq!(program.local_types[*src as usize], ValueType::I32);
                debug_assert_eq!(program.local_types[*dst as usize], ValueType::I32);
                let v = frame.locals[*src as usize] as i32;
                unsafe {
                    *locals_ptr.add(*dst as usize) = v.wrapping_add(*add) as i64 as u64;
                }
                continue;
            }
            Op::I64LocalAddSet(a, b, dst) => {
                debug_assert_eq!(program.local_types[*a as usize], ValueType::I64);
                debug_assert_eq!(program.local_types[*b as usize], ValueType::I64);
                debug_assert_eq!(program.local_types[*dst as usize], ValueType::I64);
                let av = unsafe { *locals_ptr.add(*a as usize) };
                let bv = unsafe { *locals_ptr.add(*b as usize) };
                unsafe {
                    *locals_ptr.add(*dst as usize) = av.wrapping_add(bv);
                }
                continue;
            }
            Op::I64LocalSubConstSet(src, c, dst) => {
                debug_assert_eq!(program.local_types[*src as usize], ValueType::I64);
                debug_assert_eq!(program.local_types[*dst as usize], ValueType::I64);
                let v = unsafe { *locals_ptr.add(*src as usize) };
                unsafe {
                    *locals_ptr.add(*dst as usize) = v.wrapping_sub(*c as u64);
                }
                continue;
            }
            Op::GetGlobal(g) => {
                if *g as usize >= globals.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let gv = &globals[*g as usize];
                gv.value.push_to(&mut frame.stack);
                continue;
            }
            Op::SetGlobal(g) => {
                if *g as usize >= globals.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let value = Value::pop_from(globals[*g as usize].decl.ty, &mut frame.stack)?;
                globals[*g as usize].value = value;
                continue;
            }
            Op::Drop => {
                frame.stack.pop_u64()?;
                continue;
            }
            Op::Select => {
                let condition = frame.stack.pop_i32()?;
                let val2 = frame.stack.pop_u64()?;
                let val1 = frame.stack.pop_u64()?;
                if condition != 0 {
                    frame.stack.push_u64(val1);
                } else {
                    frame.stack.push_u64(val2);
                }
                continue;
            }
            Op::I32Const(v) => {
                frame.stack.push_i32(*v);
                continue;
            }
            Op::I64Const(v) => {
                frame.stack.push_i64(*v);
                continue;
            }
            Op::I32Eqz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
                continue;
            }
            Op::I32Eq => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
                continue;
            }
            Op::I32Ne => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
                continue;
            }
            Op::I32LtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
                continue;
            }
            Op::I32LtU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
                continue;
            }
            Op::I32GtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
                continue;
            }
            Op::I32GtU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
                continue;
            }
            Op::I32LeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
                continue;
            }
            Op::I32LeU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
                continue;
            }
            Op::I32GeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
                continue;
            }
            Op::I32GeU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
                continue;
            }
            Op::I64Eqz => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
                continue;
            }
            Op::I64Eq => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
                continue;
            }
            Op::I64Ne => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
                continue;
            }
            Op::I64LtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
                continue;
            }
            Op::I64LtU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
                continue;
            }
            Op::I64GtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
                continue;
            }
            Op::I64GtU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
                continue;
            }
            Op::I64LeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
                continue;
            }
            Op::I64LeU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
                continue;
            }
            Op::I64GeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
                continue;
            }
            Op::I64GeU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
                continue;
            }
            Op::I32Add => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_add(b));
                continue;
            }
            Op::I32Sub => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_sub(b));
                continue;
            }
            Op::I32Mul => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_mul(b));
                continue;
            }
            Op::I32And => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a & b);
                continue;
            }
            Op::I32Or => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a | b);
                continue;
            }
            Op::I32Xor => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a ^ b);
                continue;
            }
            Op::I32Shl => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shl(b));
                continue;
            }
            Op::I32ShrS => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shr(b));
                continue;
            }
            Op::I32ShrU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(a.wrapping_shr(b));
                continue;
            }
            Op::I32Rotl => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.rotate_left(b));
                continue;
            }
            Op::I32Rotr => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.rotate_right(b));
                continue;
            }
            Op::I64Add => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_add(b));
                continue;
            }
            Op::I64Sub => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_sub(b));
                continue;
            }
            Op::I64Mul => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_mul(b));
                continue;
            }
            Op::I64And => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a & b);
                continue;
            }
            Op::I64Or => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a | b);
                continue;
            }
            Op::I64Xor => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a ^ b);
                continue;
            }
            Op::I64Shl => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shl(b));
                continue;
            }
            Op::I64ShrS => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shr(b));
                continue;
            }
            Op::I64ShrU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u64(a.wrapping_shr(b));
                continue;
            }
            _ => {}
        }

        #[allow(unreachable_patterns)]
        match op_ref {
            Op::Nop
            | Op::Unreachable
            | Op::StartScope(_, _)
            | Op::EndScope(_)
            | Op::If
            | Op::Else
            | Op::Br(_)
            | Op::BrIf(_)
            | Op::BrIfLocalI64Eqz(_, _)
            | Op::BrIfLocalI32GeU(_, _, _)
            | Op::BrTable(_)
            | Op::Return
            | Op::Call(_)
            | Op::CallIndirect(_, _)
            | Op::GetLocal(_)
            | Op::SetLocal(_)
            | Op::TeeLocal(_)
            | Op::I32LocalMulAddConst(_, _, _)
            | Op::I32LocalMulAddConstSet(_, _, _, _)
            | Op::I32LocalAddConstSet(_, _, _)
            | Op::I64LocalAddSet(_, _, _)
            | Op::I64LocalSubConstSet(_, _, _)
            | Op::GetGlobal(_)
            | Op::SetGlobal(_)
            | Op::Drop
            | Op::Select
            | Op::I32Const(_)
            | Op::I64Const(_)
            | Op::I32Eqz
            | Op::I32Eq
            | Op::I32Ne
            | Op::I32LtS
            | Op::I32LtU
            | Op::I32GtS
            | Op::I32GtU
            | Op::I32LeS
            | Op::I32LeU
            | Op::I32GeS
            | Op::I32GeU
            | Op::I64Eqz
            | Op::I64Eq
            | Op::I64Ne
            | Op::I64LtS
            | Op::I64LtU
            | Op::I64GtS
            | Op::I64GtU
            | Op::I64LeS
            | Op::I64LeU
            | Op::I64GeS
            | Op::I64GeU
            | Op::I32Add
            | Op::I32Sub
            | Op::I32Mul
            | Op::I32And
            | Op::I32Or
            | Op::I32Xor
            | Op::I32Shl
            | Op::I32ShrS
            | Op::I32ShrU
            | Op::I32Rotl
            | Op::I32Rotr
            | Op::I64Add
            | Op::I64Sub
            | Op::I64Mul
            | Op::I64And
            | Op::I64Or
            | Op::I64Xor
            | Op::I64Shl
            | Op::I64ShrS
            | Op::I64ShrU => unreachable!("handled by fast-path dispatch"),
            Op::TableGet(table_idx) => {
                let idx = frame.stack.pop_u32()?;
                if *table_idx as usize >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let table = &tables[*table_idx as usize];
                let table_elements = table.elements.borrow();
                if idx as usize >= table_elements.len() {
                    return Err(Fault::MemoryOutOfBounds);
                }
                match &table_elements[idx as usize] {
                    Some(value) => value.push_to(&mut frame.stack),
                    None => frame.stack.push_ref(None), // null reference
                }
            }
            Op::TableSet(table_idx) => {
                if *table_idx as usize >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let table = &mut tables[*table_idx as usize];
                let value = pop_table_value(&mut frame.stack, table.ref_type)?;

                let idx = frame.stack.pop_u32()?;
                let mut table_elements = table.elements.borrow_mut();
                if idx as usize >= table_elements.len() {
                    return Err(Fault::MemoryOutOfBounds);
                }
                table_elements[idx as usize] = Some(value);
                table.bump_generation();
            }
            Op::TableInit(table_idx, elem_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let table = tables
                    .get_mut(*table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let segment = element_segments
                    .get(*elem_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let segment_values = segment.as_deref().unwrap_or(&[]);

                let (src_start, src_end) = validate_range(src, len, segment_values.len())?;
                let mut table_elements = table.elements.borrow_mut();
                let (dst_start, _) = validate_range(dst, len, table_elements.len())?;
                for (i, value) in segment_values[src_start..src_end].iter().enumerate() {
                    table_elements[dst_start + i] = Some(*value);
                }
                if len != 0 {
                    table.bump_generation();
                }
            }
            Op::ElemDrop(elem_idx) => {
                let segment = element_segments
                    .get_mut(*elem_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                *segment = None;
            }
            Op::TableCopy(dst_table_idx, src_table_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let src_table_idx = *src_table_idx as usize;
                let dst_table_idx = *dst_table_idx as usize;
                if src_table_idx >= tables.len() || dst_table_idx >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }

                if tables[src_table_idx].ref_type != tables[dst_table_idx].ref_type {
                    return Err(Fault::InvalidRefType);
                }

                if src_table_idx == dst_table_idx {
                    let table = &mut tables[src_table_idx];
                    let mut table_elements = table.elements.borrow_mut();
                    let (src_start, src_end) = validate_range(src, len, table_elements.len())?;
                    let (dst_start, _) = validate_range(dst, len, table_elements.len())?;
                    let copied = table_elements[src_start..src_end].to_vec();
                    for (i, value) in copied.into_iter().enumerate() {
                        table_elements[dst_start + i] = value;
                    }
                    if len != 0 {
                        table.bump_generation();
                    }
                } else {
                    let copied = {
                        let src_elements = tables[src_table_idx].elements.borrow();
                        let (src_start, src_end) = validate_range(src, len, src_elements.len())?;
                        src_elements[src_start..src_end].to_vec()
                    };
                    let dst_table = &mut tables[dst_table_idx];
                    let mut dst_elements = dst_table.elements.borrow_mut();
                    let (dst_start, _) = validate_range(dst, len, dst_elements.len())?;
                    for (i, value) in copied.into_iter().enumerate() {
                        dst_elements[dst_start + i] = value;
                    }
                    if len != 0 {
                        dst_table.bump_generation();
                    }
                }
            }
            Op::TableGrow(table_idx) => {
                let delta = frame.stack.pop_u32()?;
                let table = tables
                    .get_mut(*table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let fill_value = pop_table_value(&mut frame.stack, table.ref_type)?;

                let mut table_elements = table.elements.borrow_mut();
                let old_len = table_elements.len();
                let Some(new_len) = old_len.checked_add(delta as usize) else {
                    frame.stack.push_i32(-1);
                    continue;
                };
                let exceeds_limit = new_len > u32::MAX as usize
                    || table.limits.1.is_some_and(|max| new_len > max as usize);
                if exceeds_limit {
                    frame.stack.push_i32(-1);
                    continue;
                }
                table_elements.resize(new_len, Some(fill_value));
                table.bump_generation();
                frame.stack.push_u32(old_len as u32);
            }
            Op::TableSize(table_idx) => {
                let table = tables
                    .get(*table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let table_len = table.elements.borrow().len();
                frame.stack.push_u32(table_len as u32);
            }
            Op::TableFill(table_idx) => {
                let len = frame.stack.pop_u32()?;
                let table = tables
                    .get_mut(*table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let value = pop_table_value(&mut frame.stack, table.ref_type)?;
                let dst = frame.stack.pop_u32()?;

                let mut table_elements = table.elements.borrow_mut();
                let (dst_start, dst_end) = validate_range(dst, len, table_elements.len())?;
                for slot in &mut table_elements[dst_start..dst_end] {
                    *slot = Some(value);
                }
                if len != 0 {
                    table.bump_generation();
                }
            }
            Op::LoadI32(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let data = memory.data();
                let end = addr.wrapping_add(4);
                if addr > end || end > data.len() {
                    return Err(Fault::MemoryOutOfBounds);
                }
                let value =
                    i32::from_le_bytes(unsafe { *(data.as_ptr().add(addr) as *const [u8; 4]) });
                frame.stack.push_i32(value);
            }
            Op::LoadI64(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u64(data, addr)? as i64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u64(data, addr)? as i64;
                    value
                };
                frame.stack.push_i64(value);
            }
            Op::LoadF32(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = f32::from_bits(mem_read_u32(data, addr)?);
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = f32::from_bits(mem_read_u32(data, addr)?);
                    value
                };
                frame.stack.push_f32(value);
            }
            Op::LoadF64(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = f64::from_bits(mem_read_u64(data, addr)?);
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = f64::from_bits(mem_read_u64(data, addr)?);
                    value
                };
                frame.stack.push_f64(value);
            }

            // Extending load, signed
            Op::Load8SE(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u8(data, addr)? as i8 as i32;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u8(data, addr)? as i8 as i32;
                    value
                };
                frame.stack.push_i32(value);
            }
            Op::Load16Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u16(data, addr)? as i16 as i32;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u16(data, addr)? as i16 as i32;
                    value
                };
                frame.stack.push_i32(value);
            }
            Op::Load8I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u8(data, addr)? as i8 as i64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u8(data, addr)? as i8 as i64;
                    value
                };
                frame.stack.push_i64(value);
            }
            Op::Load16I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u16(data, addr)? as i16 as i64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u16(data, addr)? as i16 as i64;
                    value
                };
                frame.stack.push_i64(value);
            }
            Op::Load32I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u32(data, addr)? as i32 as i64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u32(data, addr)? as i32 as i64;
                    value
                };
                frame.stack.push_i64(value);
            }

            // Extending load, unsigned
            Op::Load8Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u8(data, addr)? as u32;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u8(data, addr)? as u32;
                    value
                };
                frame.stack.push_u32(value);
            }
            Op::Load16Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u16(data, addr)? as u32;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u16(data, addr)? as u32;
                    value
                };
                frame.stack.push_u32(value);
            }
            Op::Load8I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u8(data, addr)? as u64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u8(data, addr)? as u64;
                    value
                };
                frame.stack.push_u64(value);
            }
            Op::Load16I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u16(data, addr)? as u64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u16(data, addr)? as u64;
                    value
                };
                frame.stack.push_u64(value);
            }
            Op::Load32I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let value = if memarg.memory == 0 {
                    let data = unsafe { &*memory0_data_ptr };
                    let value = mem_read_u32(data, addr)? as u64;
                    value
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data();
                    let value = mem_read_u32(data, addr)? as u64;
                    value
                };
                frame.stack.push_u64(value);
            }
            Op::StoreI32(addr) => {
                let value = frame.stack.pop_i32()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u32(data, addr, value as u32)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u32(data, addr, value as u32)?;
                }
            }
            Op::StoreI64(addr) => {
                let value = frame.stack.pop_i64()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u64(data, addr, value as u64)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u64(data, addr, value as u64)?;
                }
            }
            Op::StoreF32(addr) => {
                let value = frame.stack.pop_f32()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u32(data, addr, value.to_bits())?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u32(data, addr, value.to_bits())?;
                }
            }
            Op::StoreF64(addr) => {
                let value = frame.stack.pop_f64()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u64(data, addr, value.to_bits())?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u64(data, addr, value.to_bits())?;
                }
            }

            // Silently narrow the width of the value
            Op::Store8_32(addr) => {
                let value = frame.stack.pop_i32()? as u8;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u8(data, addr, value)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u8(data, addr, value)?;
                }
            }
            Op::Store16_32(addr) => {
                let value = frame.stack.pop_i32()? as u16;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u16(data, addr, value)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u16(data, addr, value)?;
                }
            }
            Op::Store8_64(addr) => {
                let value = frame.stack.pop_i64()? as u8;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u8(data, addr, value)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u8(data, addr, value)?;
                }
            }
            Op::Store16_64(addr) => {
                let value = frame.stack.pop_i64()? as u16;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u16(data, addr, value)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u16(data, addr, value)?;
                }
            }
            Op::Store32_64(addr) => {
                let value = frame.stack.pop_i64()? as u32;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                if memarg.memory == 0 {
                    let data = unsafe { &mut *memory0_data_ptr };
                    mem_write_u32(data, addr, value)?;
                } else {
                    let memory = memories
                        .get_mut(memarg.memory as usize)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let data = memory.data_mut();
                    mem_write_u32(data, addr, value)?;
                }
            }

            Op::I32Const(v) => {
                frame.stack.push_i32(*v);
            }
            Op::I64Const(v) => {
                frame.stack.push_i64(*v);
            }
            Op::F32Const(v) => {
                frame.stack.push_u32(v.to_bits());
            }
            Op::F64Const(v) => {
                frame.stack.push_u64(v.to_bits());
            }
            Op::MemorySize(mem_idx) => {
                let memory = memories
                    .get_mut(*mem_idx as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let size_in_bytes = memory.size();
                let size_in_pages = size_in_bytes / WASM_PAGE_SIZE;
                frame.stack.push_u32(size_in_pages as u32);
            }
            Op::MemoryGrow(mem_idx) => {
                let delta = frame.stack.pop_i32()?;
                let memory = memories
                    .get_mut(*mem_idx as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                if delta < 0 {
                    frame.stack.push_i32(-1);
                } else {
                    let current_size = memory.size();
                    let old_page_count = current_size / WASM_PAGE_SIZE;
                    let Some(growth_size) = (delta as usize).checked_mul(WASM_PAGE_SIZE) else {
                        frame.stack.push_i32(-1);
                        continue;
                    };
                    let Some(new_size) = current_size.checked_add(growth_size) else {
                        frame.stack.push_i32(-1);
                        continue;
                    };
                    if memory.grow(new_size).is_ok() {
                        frame.stack.push_i32(old_page_count as i32);
                        if *mem_idx == 0 {
                            memory0_data_ptr = memories[0].data_mut();
                        }
                    } else {
                        frame.stack.push_i32(-1);
                    }
                }
            }
            Op::MemoryInit(data_idx, mem_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let memory = memories
                    .get_mut(*mem_idx as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let data_segment = data_segments
                    .get(*data_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let data_bytes = data_segment.as_deref().unwrap_or(&[]);

                let (src_start, src_end) = validate_range(src, len, data_bytes.len())?;
                let (dst_start, dst_end) = validate_range(dst, len, memory.size())?;
                memory.data_mut()[dst_start..dst_end]
                    .copy_from_slice(&data_bytes[src_start..src_end]);
            }
            Op::DataDrop(data_idx) => {
                let data_segment = data_segments
                    .get_mut(*data_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                *data_segment = None;
            }
            Op::MemoryCopy(dst_mem_idx, src_mem_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let dst_mem_idx = *dst_mem_idx as usize;
                let src_mem_idx = *src_mem_idx as usize;
                if dst_mem_idx >= memories.len() || src_mem_idx >= memories.len() {
                    return Err(Fault::MemoryOutOfBounds);
                }

                if dst_mem_idx == src_mem_idx {
                    let memory = memories
                        .get_mut(dst_mem_idx)
                        .ok_or(Fault::MemoryOutOfBounds)?;
                    let memory_size = memory.size();
                    let (src_start, src_end) = validate_range(src, len, memory_size)?;
                    let (dst_start, _) = validate_range(dst, len, memory_size)?;
                    memory.data_mut().copy_within(src_start..src_end, dst_start);
                } else if dst_mem_idx < src_mem_idx {
                    let (left, right) = memories.split_at_mut(src_mem_idx);
                    let dst_memory = &mut left[dst_mem_idx];
                    let src_memory = &mut right[0];
                    let (src_start, src_end) = validate_range(src, len, src_memory.size())?;
                    let (dst_start, dst_end) = validate_range(dst, len, dst_memory.size())?;
                    let src_bytes = src_memory.data()[src_start..src_end].to_vec();
                    dst_memory.data_mut()[dst_start..dst_end].copy_from_slice(&src_bytes);
                } else {
                    let (left, right) = memories.split_at_mut(dst_mem_idx);
                    let src_memory = &mut left[src_mem_idx];
                    let dst_memory = &mut right[0];
                    let (src_start, src_end) = validate_range(src, len, src_memory.size())?;
                    let (dst_start, dst_end) = validate_range(dst, len, dst_memory.size())?;
                    let src_bytes = src_memory.data()[src_start..src_end].to_vec();
                    dst_memory.data_mut()[dst_start..dst_end].copy_from_slice(&src_bytes);
                }
            }
            Op::MemoryFill(mem_idx) => {
                let len = frame.stack.pop_u32()?;
                let value = frame.stack.pop_i32()? as u8;
                let dst = frame.stack.pop_u32()?;

                let memory = memories
                    .get_mut(*mem_idx as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let (dst_start, dst_end) = validate_range(dst, len, memory.size())?;
                memory.data_mut()[dst_start..dst_end].fill(value);
            }
            Op::I32Eqz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
            }
            Op::I32Eq => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::I32Ne => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::I32LtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I32LtU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I32GtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I32GtU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I32LeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I32LeU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I32GeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I32GeU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I64Eqz => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
            }
            Op::I64Eq => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::I64Ne => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::I64LtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I64LtU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I64GtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I64GtU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I64LeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I64LeU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I64GeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I64GeU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::F32Eq => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::F32Ne => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::F32Lt => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::F32Gt => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::F32Le => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::F32Ge => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::F64Eq => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::F64Ne => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::F64Lt => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::F64Gt => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::F64Le => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::F64Ge => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I32Clz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_u32(value.leading_zeros());
            }
            Op::I32Ctz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_u32(value.trailing_zeros());
            }
            Op::I32Popcnt => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_u32(value.count_ones());
            }
            Op::I32Add => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_add(b));
            }
            Op::I32Sub => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_sub(b));
            }
            Op::I32Mul => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_mul(b));
            }
            Op::I32DivS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                match a.checked_div(b) {
                    Some(result) => frame.stack.push_i32(result),
                    None => {
                        if b == 0 {
                            return Err(Fault::IntegerDivisionByZero);
                        } else {
                            return Err(Fault::IntegerOverflow);
                        }
                    }
                }
            }
            Op::I32DivU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                match a.checked_div(b) {
                    Some(result) => frame.stack.push_u32(result),
                    None => return Err(Fault::IntegerDivisionByZero),
                }
            }
            Op::I32RemS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                match a.checked_rem(b) {
                    Some(result) => frame.stack.push_i32(result),
                    None => {
                        if b == 0 {
                            return Err(Fault::IntegerDivisionByZero);
                        } else {
                            // i32::MIN % -1 = 0 by WASM spec
                            frame.stack.push_i32(0);
                        }
                    }
                }
            }
            Op::I32RemU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                match a.checked_rem(b) {
                    Some(result) => frame.stack.push_u32(result),
                    None => return Err(Fault::IntegerDivisionByZero),
                }
            }
            Op::I32And => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a & b);
            }
            Op::I32Or => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a | b);
            }
            Op::I32Xor => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a ^ b);
            }
            Op::I32Shl => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shl(b));
            }
            Op::I32ShrS => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shr(b));
            }
            Op::I32ShrU => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_u32()?;
                frame.stack.push_u32(a.wrapping_shr(b));
            }
            Op::I32Rotl => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.rotate_left(b));
            }
            Op::I32Rotr => {
                let b = frame.stack.pop_u32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.rotate_right(b));
            }
            Op::I64Clz => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value.leading_zeros() as i64);
            }
            Op::I64Ctz => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value.trailing_zeros() as i64);
            }
            Op::I64Popcnt => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value.count_ones() as i64);
            }
            Op::I64Add => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_add(b));
            }
            Op::I64Sub => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_sub(b));
            }
            Op::I64Mul => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_mul(b));
            }
            Op::I64DivS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                match a.checked_div(b) {
                    Some(result) => frame.stack.push_i64(result),
                    None => {
                        if b == 0 {
                            return Err(Fault::IntegerDivisionByZero);
                        } else {
                            return Err(Fault::IntegerOverflow);
                        }
                    }
                }
            }
            Op::I64DivU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                match a.checked_div(b) {
                    Some(result) => frame.stack.push_u64(result),
                    None => return Err(Fault::IntegerDivisionByZero),
                }
            }
            Op::I64RemS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                match a.checked_rem(b) {
                    Some(result) => frame.stack.push_i64(result),
                    None => {
                        if b == 0 {
                            return Err(Fault::IntegerDivisionByZero);
                        } else {
                            // i64::MIN % -1 = 0 by WASM spec
                            frame.stack.push_i64(0);
                        }
                    }
                }
            }
            Op::I64RemU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                match a.checked_rem(b) {
                    Some(result) => frame.stack.push_u64(result),
                    None => return Err(Fault::IntegerDivisionByZero),
                }
            }
            Op::I64And => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a & b);
            }
            Op::I64Or => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a | b);
            }
            Op::I64Xor => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a ^ b);
            }
            Op::I64Shl => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shl(b as u32));
            }
            Op::I64ShrS => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shr(b as u32));
            }
            Op::I64ShrU => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_u64()?;
                frame.stack.push_u64(a.wrapping_shr(b as u32));
            }
            Op::I64Rotl => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.rotate_left(b as u32));
            }
            Op::I64Rotr => {
                let b = frame.stack.pop_u64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.rotate_right(b as u32));
            }
            Op::F32Abs => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.abs());
            }
            Op::F32Neg => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(-value);
            }
            Op::F32Ceil => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.ceil());
            }
            Op::F32Floor => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.floor());
            }
            Op::F32Trunc => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.trunc());
            }
            Op::F32Nearest => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.round_ties_even());
            }
            Op::F32Sqrt => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f32(value.sqrt());
            }
            Op::F32Add => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(a + b);
            }
            Op::F32Sub => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(a - b);
            }
            Op::F32Mul => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(a * b);
            }
            Op::F32Div => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(a / b);
            }
            Op::F32Min => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                let result = if a.is_nan() || b.is_nan() {
                    f32::NAN
                } else {
                    a.min(b)
                };
                frame.stack.push_f32(result);
            }
            Op::F32Max => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                let result = if a.is_nan() || b.is_nan() {
                    f32::NAN
                } else {
                    a.max(b)
                };
                frame.stack.push_f32(result);
            }
            Op::F32Copysign => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(a.copysign(b));
            }
            Op::F64Add => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(a + b);
            }
            Op::F64Sub => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(a - b);
            }
            Op::F64Mul => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(a * b);
            }
            Op::F64Div => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(a / b);
            }
            Op::F64Min => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                let result = if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.min(b)
                };
                frame.stack.push_f64(result);
            }
            Op::F64Max => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                let result = if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.max(b)
                };
                frame.stack.push_f64(result);
            }
            Op::F64Copysign => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(a.copysign(b));
            }
            Op::F64Abs => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.abs());
            }
            Op::F64Neg => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(-value);
            }
            Op::F64Ceil => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.ceil());
            }
            Op::F64Floor => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.floor());
            }
            Op::F64Trunc => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.trunc());
            }
            Op::F64Nearest => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.round_ties_even());
            }
            Op::F64Sqrt => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f64(value.sqrt());
            }
            Op::I32WrapI64 => {
                let value = frame.stack.pop_i64()?;
                // Turn to i32, wrapping around if necessary
                // TODO: I think this is wrong
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF32S => {
                let value = frame.stack.pop_f32()?;
                let result = trunc_f32_to_i32(value)?;
                frame.stack.push_i32(result);
            }
            Op::I32TruncF32U => {
                let value = frame.stack.pop_f32()?;
                let result = trunc_f32_to_u32(value)?;
                frame.stack.push_u32(result);
            }
            Op::I32TruncF64S => {
                let value = frame.stack.pop_f64()?;
                let result = trunc_f64_to_i32(value)?;
                frame.stack.push_i32(result);
            }
            Op::I32TruncF64U => {
                let value = frame.stack.pop_f64()?;
                let result = trunc_f64_to_u32(value)?;
                frame.stack.push_u32(result);
            }
            Op::I64ExtendI32S => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i64(value as i64);
            }
            Op::I64ExtendI32U => {
                let value = frame.stack.pop_u32()?;
                frame.stack.push_u64(value as u64);
            }
            Op::I64TruncF32S => {
                let value = frame.stack.pop_f32()?;
                let result = trunc_f32_to_i64(value)?;
                frame.stack.push_i64(result);
            }
            Op::I64TruncF32U => {
                let value = frame.stack.pop_f32()?;
                let result = trunc_f32_to_u64(value)?;
                frame.stack.push_u64(result);
            }
            Op::I64TruncF64S => {
                let value = frame.stack.pop_f64()?;
                let result = trunc_f64_to_i64(value)?;
                frame.stack.push_i64(result);
            }
            Op::I64TruncF64U => {
                let value = frame.stack.pop_f64()?;
                let result = trunc_f64_to_u64(value)?;
                frame.stack.push_u64(result);
            }

            // Saturating truncation operations
            Op::I32TruncSatF32S => {
                let value = frame.stack.pop_f32()?;
                let result = if value.is_nan() {
                    0
                } else if value <= (i32::MIN as f32) {
                    i32::MIN
                } else if value >= (i32::MAX as f32) {
                    i32::MAX
                } else {
                    value as i32
                };
                frame.stack.push_i32(result);
            }
            Op::I32TruncSatF32U => {
                let value = frame.stack.pop_f32()?;
                let result = if value.is_nan() || value < 0.0 {
                    0
                } else if value >= (u32::MAX as f32) {
                    u32::MAX
                } else {
                    value as u32
                };
                frame.stack.push_u32(result);
            }
            Op::I32TruncSatF64S => {
                let value = frame.stack.pop_f64()?;
                let result = if value.is_nan() {
                    0
                } else if value <= (i32::MIN as f64) {
                    i32::MIN
                } else if value >= (i32::MAX as f64) {
                    i32::MAX
                } else {
                    value as i32
                };
                frame.stack.push_i32(result);
            }
            Op::I32TruncSatF64U => {
                let value = frame.stack.pop_f64()?;
                let result = if value.is_nan() || value < 0.0 {
                    0
                } else if value >= (u32::MAX as f64) {
                    u32::MAX
                } else {
                    value as u32
                };
                frame.stack.push_u32(result);
            }
            Op::I64TruncSatF32S => {
                let value = frame.stack.pop_f32()?;
                let result = if value.is_nan() {
                    0
                } else if value <= (i64::MIN as f32) {
                    i64::MIN
                } else if value >= (i64::MAX as f32) {
                    i64::MAX
                } else {
                    value as i64
                };
                frame.stack.push_i64(result);
            }
            Op::I64TruncSatF32U => {
                let value = frame.stack.pop_f32()?;
                let result = if value.is_nan() || value < 0.0 {
                    0
                } else if value >= (u64::MAX as f32) {
                    u64::MAX
                } else {
                    value as u64
                };
                frame.stack.push_u64(result);
            }
            Op::I64TruncSatF64S => {
                let value = frame.stack.pop_f64()?;
                let result = if value.is_nan() {
                    0
                } else if value <= (i64::MIN as f64) {
                    i64::MIN
                } else if value >= (i64::MAX as f64) {
                    i64::MAX
                } else {
                    value as i64
                };
                frame.stack.push_i64(result);
            }
            Op::I64TruncSatF64U => {
                let value = frame.stack.pop_f64()?;
                let result = if value.is_nan() || value < 0.0 {
                    0
                } else if value >= (u64::MAX as f64) {
                    u64::MAX
                } else {
                    value as u64
                };
                frame.stack.push_u64(result);
            }

            Op::F32ConvertI32S => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI32U => {
                let value = frame.stack.pop_u32()?;
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI64S => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI64U => {
                let value = frame.stack.pop_u64()?;
                frame.stack.push_f32(value as f32);
            }
            Op::F32DemoteF64 => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_f32(value as f32);
            }
            Op::F64ConvertI32S => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI32U => {
                let value = frame.stack.pop_u32()?;
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI64S => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI64U => {
                let value = frame.stack.pop_u64()?;
                frame.stack.push_f64(value as f64);
            }
            Op::F64PromoteF32 => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_f64(value as f64);
            }
            Op::I32ReinterpretF32 => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_u32(value.to_bits());
            }
            Op::I64ReinterpretF64 => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_u64(value.to_bits());
            }
            Op::F32ReinterpretI32 => {
                let value = frame.stack.pop_u32()?;
                frame.stack.push_f32(f32::from_bits(value));
            }
            Op::F64ReinterpretI64 => {
                let value = frame.stack.pop_u64()?;
                frame.stack.push_f64(f64::from_bits(value));
            }
            Op::I32Extend8S => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(value as i8 as i32);
            }
            Op::I32Extend16S => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(value as i16 as i32);
            }
            Op::I64Extend8S => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value as i8 as i64);
            }
            Op::I64Extend16S => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value as i16 as i64);
            }
            Op::I64Extend32S => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i64(value as i32 as i64);
            }

            // Reference types operations
            Op::RefNull(ref_type) => match ref_type {
                crate::ValueType::FuncRef | crate::ValueType::ExternRef => {
                    frame.stack.push_ref(None);
                }
                _ => return Err(Fault::InvalidRefType),
            },
            Op::RefFunc(func_index) => {
                // Validate func_index exists in the module's function index space
                if (*func_index as usize) >= functions.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                frame.stack.push_ref(Some(encode_func_ref(FuncRef {
                    module_id: current_module_id,
                    func_idx: *func_index,
                })));
            }
            Op::RefIsNull => {
                let ref_val = frame.stack.pop_ref()?;
                let is_null = if ref_val.is_none() { 1 } else { 0 };
                frame.stack.push_i32(is_null);
            }
            Op::RefAsNonNull => {
                let ref_val = frame.stack.pop_ref()?;
                match ref_val {
                    Some(val) => frame.stack.push_ref(Some(val)),
                    None => return Err(Fault::NullReference),
                }
            }
            Op::RefEq => {
                let ref2 = frame.stack.pop_ref()?;
                let ref1 = frame.stack.pop_ref()?;
                let are_equal = if ref1 == ref2 { 1 } else { 0 };
                frame.stack.push_i32(are_equal);
            }
            Op::SelectT(_idx) => {
                // For now, implement same as regular select
                // TODO: Add type validation using select_types[idx]
                let condition = frame.stack.pop_i32()?;
                let val2 = frame.stack.pop_u64()?;
                let val1 = frame.stack.pop_u64()?;
                if condition != 0 {
                    frame.stack.push_u64(val1);
                } else {
                    frame.stack.push_u64(val2);
                }
            }
        }
    }
}

#[inline(always)]
fn mem_read_u8(data: &[u8], addr: usize) -> Result<u8, Fault> {
    if addr >= data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe { Ok(*data.as_ptr().add(addr)) }
}

#[inline(always)]
fn mem_read_u16(data: &[u8], addr: usize) -> Result<u16, Fault> {
    let end = addr.wrapping_add(2);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        let bytes = *(data.as_ptr().add(addr) as *const [u8; 2]);
        Ok(u16::from_le_bytes(bytes))
    }
}

#[inline(always)]
fn mem_read_u32(data: &[u8], addr: usize) -> Result<u32, Fault> {
    let end = addr.wrapping_add(4);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        let bytes = *(data.as_ptr().add(addr) as *const [u8; 4]);
        Ok(u32::from_le_bytes(bytes))
    }
}

#[inline(always)]
fn mem_read_u64(data: &[u8], addr: usize) -> Result<u64, Fault> {
    let end = addr.wrapping_add(8);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        let bytes = *(data.as_ptr().add(addr) as *const [u8; 8]);
        Ok(u64::from_le_bytes(bytes))
    }
}

#[inline(always)]
fn mem_write_u8(data: &mut [u8], addr: usize, value: u8) -> Result<(), Fault> {
    if addr >= data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        *data.as_mut_ptr().add(addr) = value;
    }
    Ok(())
}

#[inline(always)]
fn mem_write_u16(data: &mut [u8], addr: usize, value: u16) -> Result<(), Fault> {
    let end = addr.wrapping_add(2);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), data.as_mut_ptr().add(addr), 2);
    }
    Ok(())
}

#[inline(always)]
fn mem_write_u32(data: &mut [u8], addr: usize, value: u32) -> Result<(), Fault> {
    let end = addr.wrapping_add(4);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), data.as_mut_ptr().add(addr), 4);
    }
    Ok(())
}

#[inline(always)]
fn mem_write_u64(data: &mut [u8], addr: usize, value: u64) -> Result<(), Fault> {
    let end = addr.wrapping_add(8);
    if addr > end || end > data.len() {
        return Err(Fault::MemoryOutOfBounds);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), data.as_mut_ptr().add(addr), 8);
    }
    Ok(())
}

#[inline(always)]
fn adjust_memarg(stack: &mut Stack, memarg: &MemArg) -> Result<usize, Fault> {
    let base_addr = stack.pop_u32()? as usize;

    // Note: Alignment is only a "hint", we could issue a warning here, but that would just slow
    //  down the interpreter.

    memarg
        .offset
        .checked_add(base_addr)
        .ok_or(Fault::MemoryOutOfBounds)
}

#[derive(Debug, Clone)]
pub struct GlobalVar {
    pub decl: Global,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    V128(u64, u64),
    FuncRef(Option<FuncRef>),
    ExternRef(Option<u32>),
    Unit,
}

fn encode_func_ref(func_ref: FuncRef) -> u64 {
    ((func_ref.module_id as u64) << 32) | func_ref.func_idx as u64
}

fn decode_func_ref(raw: u64) -> FuncRef {
    FuncRef {
        module_id: (raw >> 32) as u32,
        func_idx: raw as u32,
    }
}

impl Value {
    #[inline(always)]
    pub(crate) fn type_of(&self) -> ValueType {
        match self {
            Value::I32(_) => ValueType::I32,
            Value::I64(_) => ValueType::I64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::V128(_, _) => ValueType::V128,
            Value::FuncRef(_) => ValueType::FuncRef,
            Value::ExternRef(_) => ValueType::ExternRef,
            Value::Unit => ValueType::Unit,
        }
    }

    #[inline(always)]
    pub fn pop_from(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => Value::Unit,
            ValueType::I32 => Value::I32(stack.pop_i32()?),
            ValueType::I64 => Value::I64(stack.pop_i64()?),
            ValueType::F32 => Value::F32(stack.pop_f32()?),
            ValueType::F64 => Value::F64(stack.pop_f64()?),
            ValueType::V128 => {
                let l = stack.pop_u64()?;
                let r = stack.pop_u64()?;
                Value::V128(l, r)
            }
            ValueType::FuncRef => Value::FuncRef(stack.pop_ref()?.map(decode_func_ref)),
            ValueType::ExternRef => Value::ExternRef(stack.pop_ref()?.map(|v| v as u32)),
        })
    }

    #[inline(always)]
    pub fn top_of(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => Value::Unit,
            ValueType::I32 => Value::I32(stack.top_i32()?),
            ValueType::I64 => Value::I64(stack.top_i64()?),
            ValueType::F32 => Value::F32(stack.top_f32()?),
            ValueType::F64 => Value::F64(stack.top_f64()?),
            ValueType::V128 => {
                let l = stack.top_u64()?;
                let r = stack.top_u64()?;
                Value::V128(l, r)
            }
            ValueType::FuncRef => {
                let raw = stack.top_u64()?;
                if raw == u64::MAX {
                    Value::FuncRef(None)
                } else {
                    Value::FuncRef(Some(decode_func_ref(raw)))
                }
            }
            ValueType::ExternRef => {
                let raw = stack.top_u64()?;
                if raw == u64::MAX {
                    Value::ExternRef(None)
                } else {
                    Value::ExternRef(Some(raw as u32))
                }
            }
        })
    }

    #[inline(always)]
    pub fn push_to(&self, stack: &mut Stack) {
        match self {
            Value::I32(v) => stack.push_i32(*v),
            Value::I64(v) => stack.push_i64(*v),
            Value::F32(v) => stack.push_f32(*v),
            Value::F64(v) => stack.push_f64(*v),
            Value::V128(l, r) => {
                stack.push_u64(*l);
                stack.push_u64(*r);
            }
            Value::FuncRef(v) => stack.push_ref(v.map(encode_func_ref)),
            Value::ExternRef(v) => stack.push_ref(v.map(|v| v as u64)),
            Value::Unit => {}
        }
    }

    #[inline(always)]
    pub fn push_raw_to(ty: ValueType, val: u64, stack: &mut Stack) {
        match ty {
            ValueType::V128 => unreachable!("V128 not supported in raw push yet"),
            ValueType::Unit => {}
            _ => stack.push_u64(val),
        }
    }

    pub fn from_raw(ty: ValueType, val: u64) -> Self {
        match ty {
            ValueType::I32 => Value::I32(val as i32),
            ValueType::I64 => Value::I64(val as i64),
            ValueType::F32 => Value::F32(f32::from_bits(val as u32)),
            ValueType::F64 => Value::F64(f64::from_bits(val)),
            ValueType::V128 => unreachable!("V128 not supported in raw values"),
            ValueType::FuncRef => Value::FuncRef(if val == u64::MAX {
                None
            } else {
                Some(decode_func_ref(val))
            }),
            ValueType::ExternRef => Value::ExternRef(if val == u64::MAX {
                None
            } else {
                Some(val as u32)
            }),
            ValueType::Unit => Value::Unit,
        }
    }

    pub fn to_raw(&self) -> u64 {
        match self {
            Value::I32(v) => *v as i64 as u64,
            Value::I64(v) => *v as u64,
            Value::F32(v) => v.to_bits() as u64,
            Value::F64(v) => v.to_bits(),
            Value::V128(_, _) => unreachable!("V128 not supported in raw values"),
            Value::FuncRef(v) => match v {
                Some(fr) => encode_func_ref(*fr),
                None => u64::MAX,
            },
            Value::ExternRef(v) => match v {
                Some(er) => *er as u64,
                None => u64::MAX,
            },
            Value::Unit => 0,
        }
    }

    #[inline(always)]
    pub fn pop_raw_from(ty: ValueType, stack: &mut Stack) -> Result<u64, Fault> {
        match ty {
            ValueType::V128 => Err(Fault::Unreachable),
            ValueType::Unit => Ok(0),
            _ => stack.pop_u64(),
        }
    }

    #[inline(always)]
    pub fn top_raw_of(ty: ValueType, stack: &mut Stack) -> Result<u64, Fault> {
        match ty {
            ValueType::V128 => Err(Fault::Unreachable),
            ValueType::Unit => Ok(0),
            _ => stack.top_u64(),
        }
    }

    /// Equality comparison where NaN == NaN, for tests.
    pub fn eq_w_nan(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::F32(a), Value::F32(b)) => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (Value::F64(a), Value::F64(b)) => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            _ => self == other,
        }
    }
}

/// Evaluate an initialization expression with access to imported globals
pub(crate) fn eval_init_expr(
    program: &[u8],
    return_type: ValueType,
    imported_globals: &[GlobalVar],
    types: &[crate::FuncType],
    functions: &[usize],
    current_module_owner_id: u32,
) -> Result<Value, Fault> {
    let mut expression = program.to_vec();
    // Module::get_expr returns expression bytes without the terminating END opcode.
    // Always append it here; immediate bytes can legitimately be 0x0b (e.g. ref.func 11).
    expression.push(0x0b);
    let mut const_program = decode(&expression).unwrap();
    const_program.return_types = vec![return_type];
    let mut global_exec_frame = Frame::for_owned_program(vec![], Rc::new(const_program));
    // This little fragment, it doesn't get much memory and doesn't get *any* globals.
    // TODO: I don't actually know what a reasonable amount of memory is, so we'll just default
    //   to one page.
    let mut const_prg_memory_vec = vec![0; WASM_PAGE_SIZE];
    let mut const_prg_memory = SliceMemory::new(&mut const_prg_memory_vec);
    let mut const_prg_memories: [&mut dyn Memory; 1] = [&mut const_prg_memory];

    // Create a globals context that includes imported globals
    let mut const_prg_globals = imported_globals.to_vec();

    // In this case the expectation is we run out of instructions, and the stack contains the return
    // value.
    let mut const_prg_tables = vec![];
    let mut const_prg_element_segments = vec![];
    let mut const_prg_data_segments = vec![];
    let result = execute::<true>(
        &mut global_exec_frame,
        &mut const_prg_memories,
        &mut const_prg_globals,
        &mut const_prg_tables,
        &mut const_prg_element_segments,
        &mut const_prg_data_segments,
        EXPR_TICK_LIMIT,
        types,
        functions,
        current_module_owner_id,
        current_module_owner_id,
    )?;
    let (result, _) = result;
    // Must be `ProgramEnd`, or there's a bug, and that's UnexpectedResult
    match result {
        Continuation::ProgramEnd | Continuation::DoneReturn => {}
        _ => return Err(Fault::UnexpectedResult(Box::new(result))),
    }

    Value::pop_from(return_type, &mut global_exec_frame.stack)
}

#[derive(Debug)]
pub enum ExecError {
    LinkageError(LinkError),
    ExecutionFault(Fault),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Complete,
    Paused,
}

impl Display for ExecError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::LinkageError(e) => write!(f, "Linkage error: {e}"),
            ExecError::ExecutionFault(e) => write!(f, "Execution fault: {e}"),
        }
    }
}

impl Error for ExecError {}

#[inline(always)]
fn execute_module_frame<const TRACK_TICKS: bool>(
    module_id: u32,
    frame: &mut Frame,
    instance: &mut Instance,
    max_ticks: usize,
) -> Result<(Continuation, usize), Fault> {
    let module_owner_id = instance.owner_id;
    let function_type_mapping = &instance.function_type_mapping;
    let (memories, globals, tables, element_segments, data_segments, module) = (
        &mut instance.memories,
        &mut instance.globals,
        &mut instance.tables,
        &mut instance.element_segments,
        &mut instance.data_segments,
        &instance.module,
    );
    // Memory instructions are validated at module level, but keep an empty fallback memory so
    // malformed modules trap cleanly instead of panicking.
    match memories.len() {
        0 => {
            let mut fallback_memory = crate::VectorMemory::new(0, None);
            let mut fallback_memories: [&mut dyn Memory; 1] = [&mut fallback_memory];
            execute::<TRACK_TICKS>(
                frame,
                &mut fallback_memories,
                globals,
                tables,
                element_segments,
                data_segments,
                max_ticks,
                &module.types,
                function_type_mapping,
                module_id,
                module_owner_id,
            )
        }
        1 => {
            let memory = memories.get_mut(0).unwrap();
            let mut memory_refs: [&mut dyn Memory; 1] = [memory];
            execute::<TRACK_TICKS>(
                frame,
                &mut memory_refs,
                globals,
                tables,
                element_segments,
                data_segments,
                max_ticks,
                &module.types,
                function_type_mapping,
                module_id,
                module_owner_id,
            )
        }
        _ => {
            let mut memory_refs: Vec<&mut dyn Memory> =
                memories.iter_mut().map(|m| m as &mut dyn Memory).collect();
            execute::<TRACK_TICKS>(
                frame,
                &mut memory_refs,
                globals,
                tables,
                element_segments,
                data_segments,
                max_ticks,
                &module.types,
                function_type_mapping,
                module_id,
                module_owner_id,
            )
        }
    }
}

/// Multi-module execution state that manages multiple instances and their execution.
pub struct ExecutionState {
    /// Collection of all modules/instances participating in execution
    modules: HashMap<u32, Instance>,
    /// Lookup from stable instance owner id to execution module key.
    module_owner_map: HashMap<u32, u32>,
    /// Global frame stack across all modules
    frame_stack: Vec<(u32, Frame)>, // (module_id, frame)
    /// Reusable frames for reducing allocation churn during call-heavy execution.
    recycled_frames: Vec<Frame>,
    /// Final result when all execution completes
    result: Option<Vec<Value>>,
}

impl ExecutionState {
    pub fn new() -> Self {
        ExecutionState {
            modules: HashMap::new(),
            module_owner_map: HashMap::new(),
            frame_stack: Vec::with_capacity(256),
            recycled_frames: Vec::with_capacity(256),
            result: None,
        }
    }

    #[inline(always)]
    fn recycle_frame(&mut self, frame: Frame) {
        if !matches!(frame.program_key, ProgramKey::Local { .. }) {
            return;
        }
        if self.recycled_frames.len() < FRAME_POOL_LIMIT {
            self.recycled_frames.push(frame);
        }
    }

    #[inline(always)]
    fn pop_recycled_frame_for_program_key(&mut self, program_key: ProgramKey) -> Option<Frame> {
        let len = self.recycled_frames.len();
        if len == 0 {
            return None;
        }

        if self.recycled_frames[len - 1].program_key == program_key {
            return self.recycled_frames.pop();
        }

        let idx = self.recycled_frames[..len - 1]
            .iter()
            .rposition(|f| f.program_key == program_key)?;
        Some(self.recycled_frames.swap_remove(idx))
    }

    #[inline(always)]
    fn local_program_key(module: &Instance, funcidx: u32) -> Result<ProgramKey, Fault> {
        if funcidx < module.num_imported_functions {
            return Err(Fault::GlobalIndexOutOfBounds);
        }
        let local_idx = (funcidx - module.num_imported_functions) as usize;
        module
            .programs
            .get(local_idx)
            .ok_or(Fault::GlobalIndexOutOfBounds)?;
        Ok(ProgramKey::local(module.owner_id, local_idx as u32))
    }

    fn clear_and_recycle_active_frames(&mut self) {
        while let Some((_, frame)) = self.frame_stack.pop() {
            self.recycle_frame(frame);
        }
    }

    pub fn add_module(&mut self, module_id: u32, instance: Instance) {
        self.module_owner_map.insert(instance.owner_id, module_id);
        self.modules.insert(module_id, instance);
    }

    pub fn add_module_with_imports(
        &mut self,
        module_id: u32,
        module: Module,
        resolver: &dyn ImportResolver,
    ) -> Result<(), ExecError> {
        // Create instance with import resolution
        let instance = self.create_instance_with_imports(module, resolver)?;
        self.add_module(module_id, instance);
        Ok(())
    }

    fn create_instance_with_imports(
        &self,
        module: Module,
        resolver: &dyn ImportResolver,
    ) -> Result<Instance, ExecError> {
        // Use mk_instance_with_imports for proper import resolution
        crate::instance::mk_instance_with_imports(module, resolver).map_err(ExecError::LinkageError)
    }

    fn resolve_runtime_module_key(&self, module_ref: u32) -> Option<u32> {
        if self.modules.contains_key(&module_ref) {
            Some(module_ref)
        } else {
            self.module_owner_map.get(&module_ref).copied()
        }
    }

    fn ensure_runtime_module_available(&mut self, module_ref: u32) -> Option<u32> {
        if let Some(module_key) = self.resolve_runtime_module_key(module_ref) {
            return Some(module_key);
        }
        if let Some(detached_instance) = take_detached_instance(module_ref) {
            // Detached modules use owner id as execution key.
            self.add_module(module_ref, detached_instance);
            return Some(module_ref);
        }
        None
    }

    pub fn get_module(&self, module_id: u32) -> Option<&Instance> {
        self.modules.get(&module_id)
    }

    pub fn get_module_mut(&mut self, module_id: u32) -> Option<&mut Instance> {
        self.modules.get_mut(&module_id)
    }

    pub fn instance(&self) -> Option<&Instance> {
        self.modules.get(&0)
    }

    pub fn frame_stack_len(&self) -> usize {
        self.frame_stack.len()
    }

    pub fn result(&self) -> Option<&[Value]> {
        self.result.as_deref()
    }

    pub fn take_module(&mut self, module_id: u32) -> Option<Instance> {
        if self.frame_stack.iter().any(|(id, _)| *id == module_id) {
            return None;
        }
        let instance = self.modules.remove(&module_id)?;
        self.module_owner_map.remove(&instance.owner_id);
        let owner_id = instance.owner_id;
        self.recycled_frames.retain(|frame| {
            !matches!(
                frame.program_key,
                ProgramKey::Local {
                    owner_id: frame_owner,
                    ..
                } if frame_owner == owner_id
            )
        });
        Some(instance)
    }

    pub fn into_single_module(self) -> Option<Instance> {
        if self.modules.len() == 1 {
            self.modules
                .into_iter()
                .next()
                .map(|(_, instance)| instance)
        } else {
            None
        }
    }

    pub fn prepare(
        &mut self,
        module_id: u32,
        funcidx: u32,
        args: &[Value],
    ) -> Result<(), ExecError> {
        // Allow invoking exported imported functions by creating a trampoline frame.
        {
            let instance = self
                .modules
                .get(&module_id)
                .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
            let num_imported_funcs = instance.num_imported_functions;
            if funcidx < num_imported_funcs {
                let type_idx = *instance
                    .function_type_mapping()
                    .get(funcidx as usize)
                    .ok_or(ExecError::ExecutionFault(Fault::UnresolvableTypeIndex(
                        funcidx,
                    )))?;
                let func_type = &instance.module.types[type_idx];

                for (i, (expected, actual)) in func_type.params.iter().zip(args.iter()).enumerate()
                {
                    if *expected != actual.type_of() {
                        return Err(ExecError::LinkageError(LinkError::ArgumentTypeMismatch(
                            i,
                            *expected,
                            actual.type_of(),
                        )));
                    }
                }

                let mut program = crate::decode::Program::new();
                program.return_types = func_type.results.clone();
                program.push(Op::Call(funcidx));

                let mut frame = Frame::for_owned_program(vec![], Rc::new(program));
                for arg in args {
                    arg.push_to(&mut frame.stack);
                }
                self.frame_stack.push((module_id, frame));
                return Ok(());
            }
        }

        let program_key = {
            let instance = self
                .modules
                .get(&module_id)
                .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
            Self::local_program_key(instance, funcidx).map_err(ExecError::ExecutionFault)?
        };
        let recycled_frame = self.pop_recycled_frame_for_program_key(program_key);
        let instance = self
            .modules
            .get(&module_id)
            .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
        let frame = if let Some(mut frame) = recycled_frame {
            instance
                .refill_frame_for_funcidx(funcidx, args, &mut frame)
                .map_err(ExecError::LinkageError)?;
            frame
        } else {
            instance
                .frame_for_funcidx(funcidx, args)
                .map_err(ExecError::LinkageError)?
        };

        self.frame_stack.push((module_id, frame));
        Ok(())
    }

    fn run_core<const BUDGETED: bool>(
        &mut self,
        mut ticks_left: usize,
    ) -> Result<RunResult, ExecError> {
        // Cache current module pointer across same-module dispatch iterations.
        // This avoids repeated HashMap lookups in tight call-heavy loops.
        let mut cached_module_id: Option<u32> = None;
        let mut cached_module_ptr: *mut Instance = std::ptr::null_mut();

        loop {
            if self.frame_stack.is_empty() {
                return Ok(RunResult::Complete);
            }
            if BUDGETED && ticks_left == 0 {
                return Ok(RunResult::Paused);
            }

            'execute_current: loop {
                let (current_module_id, _) = self.frame_stack.last().unwrap();
                let current_module_id = *current_module_id;

                // Execute current frame
                let result = {
                    if cached_module_id != Some(current_module_id) || cached_module_ptr.is_null() {
                        let module = self.modules.get_mut(&current_module_id).unwrap();
                        cached_module_ptr = module as *mut Instance;
                        cached_module_id = Some(current_module_id);
                    }
                    let (_, frame) = self.frame_stack.last_mut().unwrap();
                    // SAFETY: `cached_module_ptr` comes from `self.modules.get_mut`.
                    // It remains valid until the modules map is structurally modified.
                    // We invalidate the cache before any such mutation.
                    let module = unsafe { &mut *cached_module_ptr };
                    if BUDGETED {
                        execute_module_frame::<true>(
                            current_module_id,
                            frame,
                            module,
                            ticks_left.saturating_add(1),
                        )
                    } else {
                        execute_module_frame::<false>(
                            current_module_id,
                            frame,
                            module,
                            EXEC_TICK_LIMIT,
                        )
                    }
                };

                let (mut continuation, ticks_used) = match result {
                    Ok((continuation, ticks_used)) => (continuation, ticks_used),
                    Err(Fault::OutOfTicks) if BUDGETED => {
                        return Ok(RunResult::Paused);
                    }
                    Err(fault) => {
                        if let Some((_, frame)) = self.frame_stack.pop() {
                            self.recycle_frame(frame);
                        }
                        return Err(ExecError::ExecutionFault(fault));
                    }
                };

                if BUDGETED {
                    ticks_left = ticks_left.saturating_sub(ticks_used);
                }

                // Handle continuations in a loop to support recursive calls
                loop {
                    match continuation {
                        Continuation::ProgramEnd | Continuation::DoneReturn => {
                            // Handle function return
                            let (_, mut completed_frame) = self.frame_stack.pop().unwrap();
                            let return_types = completed_frame.program().return_types.clone();
                            completed_frame.scratch_values_raw.clear();

                            if let Some((_, caller_frame)) = self.frame_stack.last_mut() {
                                match return_types.as_slice() {
                                    [] => {}
                                    [ValueType::Unit] => {}
                                    [rt] => {
                                        debug_assert_ne!(*rt, ValueType::V128);
                                        let val = completed_frame
                                            .stack
                                            .pop_u64()
                                            .map_err(ExecError::ExecutionFault)?;
                                        caller_frame.stack.push_u64(val);
                                    }
                                    _ => {
                                        completed_frame
                                            .scratch_values_raw
                                            .resize(return_types.len(), 0);
                                        for (i, rt) in return_types.iter().enumerate().rev() {
                                            debug_assert_ne!(*rt, ValueType::V128);
                                            completed_frame.scratch_values_raw[i] = completed_frame
                                                .stack
                                                .pop_u64()
                                                .map_err(ExecError::ExecutionFault)?;
                                        }
                                        for (i, rt) in return_types.iter().enumerate() {
                                            debug_assert_ne!(*rt, ValueType::V128);
                                            caller_frame
                                                .stack
                                                .push_u64(completed_frame.scratch_values_raw[i]);
                                        }
                                    }
                                }
                            } else {
                                // No caller, this is the final result
                                let final_values = self.result.get_or_insert_with(Vec::new);
                                final_values.clear();
                                match return_types.as_slice() {
                                    [] => {}
                                    [ValueType::Unit] => {}
                                    [rt] => {
                                        debug_assert_ne!(*rt, ValueType::V128);
                                        let val = completed_frame
                                            .stack
                                            .pop_u64()
                                            .map_err(ExecError::ExecutionFault)?;
                                        final_values.push(Value::from_raw(*rt, val));
                                    }
                                    _ => {
                                        completed_frame
                                            .scratch_values_raw
                                            .resize(return_types.len(), 0);
                                        for (i, rt) in return_types.iter().enumerate().rev() {
                                            debug_assert_ne!(*rt, ValueType::V128);
                                            completed_frame.scratch_values_raw[i] = completed_frame
                                                .stack
                                                .pop_u64()
                                                .map_err(ExecError::ExecutionFault)?;
                                        }
                                        for (i, rt) in return_types.iter().enumerate() {
                                            debug_assert_ne!(*rt, ValueType::V128);
                                            final_values.push(Value::from_raw(
                                                *rt,
                                                completed_frame.scratch_values_raw[i],
                                            ));
                                        }
                                    }
                                }
                                self.recycle_frame(completed_frame);
                                return Ok(RunResult::Complete);
                            }
                            self.recycle_frame(completed_frame);
                            break; // Exit continuation loop to continue outer execution loop
                        }
                        Continuation::Call(funcidx) => {
                            // Function call - check if it's imported or local
                            // SAFETY: `cached_module_ptr` points at `current_module_id` instance.
                            let module_ptr = cached_module_ptr as *const Instance;
                            let module = unsafe { &*module_ptr };

                            if module.num_imported_functions == 0 {
                                // Common hot path: module has no imports, so all calls are local.
                                let local_idx = funcidx as usize;
                                if local_idx >= module.programs.len() {
                                    return Err(ExecError::ExecutionFault(
                                        Fault::GlobalIndexOutOfBounds,
                                    ));
                                }
                                let target_program_key =
                                    ProgramKey::local(module.owner_id, local_idx as u32);
                                let recycled_frame =
                                    self.pop_recycled_frame_for_program_key(target_program_key);
                                let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                                let module = unsafe { &*module_ptr };
                                let frame = if let Some(mut frame) = recycled_frame {
                                    unsafe {
                                        module.refill_frame_for_local_idx_from_stack_unchecked(
                                            local_idx,
                                            &mut current_frame.stack,
                                            &mut frame,
                                        )
                                    }
                                    .map_err(ExecError::ExecutionFault)?;
                                    frame
                                } else {
                                    unsafe {
                                        module.frame_for_local_idx_from_stack_unchecked(
                                            local_idx,
                                            &mut current_frame.stack,
                                        )
                                    }
                                    .map_err(ExecError::ExecutionFault)?
                                };
                                self.frame_stack.push((current_module_id, frame));
                                // Fast path: directly execute callee without bouncing to outer loop.
                                if BUDGETED && ticks_left == 0 {
                                    return Ok(RunResult::Paused);
                                }
                                continue 'execute_current;
                            } else if funcidx < module.num_imported_functions {
                                // This is an imported function - dispatch via continuation
                                let imported_func =
                                    module.imported_functions.get(funcidx as usize).ok_or(
                                        ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds),
                                    )?;
                                let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                                match imported_func {
                                    ImportedFunction::Host(host_func) => {
                                        // Call host function directly
                                        let result = host_func(&mut current_frame.stack)
                                            .map_err(ExecError::ExecutionFault)?;
                                        // Handle the result continuation
                                        match result {
                                            Continuation::ProgramEnd => {
                                                // Host function completed, continue execution
                                                break;
                                            }
                                            other => {
                                                // Host function returned another continuation - handle it recursively
                                                continuation = other;
                                                continue;
                                            }
                                        }
                                    }
                                    ImportedFunction::Module {
                                        module_id,
                                        func_idx,
                                    } => {
                                        // Inter-module call
                                        continuation = Continuation::CallExternal {
                                            module_id: *module_id,
                                            func_id: *func_idx,
                                            expected_type: None,
                                        };
                                        continue;
                                    }
                                }
                            } else {
                                // Local function call
                                let target_program_key = Self::local_program_key(module, funcidx)
                                    .map_err(ExecError::ExecutionFault)?;
                                let recycled_frame =
                                    self.pop_recycled_frame_for_program_key(target_program_key);
                                let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                                let module = unsafe { &*module_ptr };
                                // Create new frame for the called function
                                let frame = if let Some(mut frame) = recycled_frame {
                                    module
                                        .refill_frame_for_funcidx_from_stack(
                                            funcidx,
                                            &mut current_frame.stack,
                                            &mut frame,
                                        )
                                        .map_err(ExecError::ExecutionFault)?;
                                    frame
                                } else {
                                    module
                                        .frame_for_funcidx_from_stack(
                                            funcidx,
                                            &mut current_frame.stack,
                                        )
                                        .map_err(ExecError::ExecutionFault)?
                                };
                                self.frame_stack.push((current_module_id, frame));
                                // Fast path: directly execute callee without bouncing to outer loop.
                                if BUDGETED && ticks_left == 0 {
                                    return Ok(RunResult::Paused);
                                }
                                continue 'execute_current;
                            }
                        }
                        Continuation::CallExternal {
                            module_id,
                            func_id,
                            expected_type,
                        } => {
                            // Inter-module call
                            // This path may materialize detached modules, which can rehash `self.modules`.
                            // Invalidate cached raw pointers before potential map mutation.
                            cached_module_id = None;
                            cached_module_ptr = std::ptr::null_mut();
                            let target_module_key = self
                                .ensure_runtime_module_available(module_id)
                                .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
                            let target_module = self
                                .modules
                                .get(&target_module_key)
                                .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;

                            // Get function signature from the target module.
                            let func_index = func_id as usize;
                            if func_index >= target_module.function_type_mapping().len() {
                                return Err(ExecError::ExecutionFault(
                                    Fault::GlobalIndexOutOfBounds,
                                ));
                            }

                            let type_idx = target_module.function_type_mapping()[func_index];
                            let func_type = &target_module.module.types[type_idx];

                            if let Some((source_module_id, expected_type_idx)) = expected_type {
                                let source_module = self.modules.get(&source_module_id).ok_or(
                                    ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds),
                                )?;
                                let expected_type = source_module
                                    .module
                                    .types
                                    .get(expected_type_idx as usize)
                                    .ok_or(ExecError::ExecutionFault(
                                        Fault::UnresolvableTypeIndex(expected_type_idx),
                                    ))?;
                                if expected_type.params != func_type.params
                                    || expected_type.results != func_type.results
                                {
                                    return Err(ExecError::ExecutionFault(
                                        Fault::IndirectCallTypeMismatch,
                                    ));
                                }
                            }

                            // If the target is imported, dispatch recursively without creating a frame.
                            if func_id < target_module.num_imported_functions {
                                let imported_func = target_module
                                    .imported_functions
                                    .get(func_id as usize)
                                    .ok_or(ExecError::ExecutionFault(
                                        Fault::GlobalIndexOutOfBounds,
                                    ))?;
                                let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                                match imported_func {
                                    ImportedFunction::Host(host_func) => {
                                        let result = host_func(&mut current_frame.stack)
                                            .map_err(ExecError::ExecutionFault)?;
                                        match result {
                                            Continuation::ProgramEnd => break,
                                            other => {
                                                continuation = other;
                                                continue;
                                            }
                                        }
                                    }
                                    ImportedFunction::Module {
                                        module_id,
                                        func_idx,
                                    } => {
                                        continuation = Continuation::CallExternal {
                                            module_id: *module_id,
                                            func_id: *func_idx,
                                            expected_type: None,
                                        };
                                        continue;
                                    }
                                }
                            } else {
                                // Create new frame in target module.
                                let target_program_key =
                                    Self::local_program_key(target_module, func_id)
                                        .map_err(ExecError::ExecutionFault)?;
                                let recycled_frame =
                                    self.pop_recycled_frame_for_program_key(target_program_key);
                                let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                                let target_module = self.modules.get(&target_module_key).ok_or(
                                    ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds),
                                )?;
                                let frame = if let Some(mut frame) = recycled_frame {
                                    target_module
                                        .refill_frame_for_funcidx_from_stack(
                                            func_id,
                                            &mut current_frame.stack,
                                            &mut frame,
                                        )
                                        .map_err(ExecError::ExecutionFault)?;
                                    frame
                                } else {
                                    target_module
                                        .frame_for_funcidx_from_stack(
                                            func_id,
                                            &mut current_frame.stack,
                                        )
                                        .map_err(ExecError::ExecutionFault)?
                                };
                                self.frame_stack.push((target_module_key, frame));
                                break; // Exit continuation loop to continue outer execution loop
                            }
                        }
                        Continuation::CallHost(host_func) => {
                            // Handle host function call
                            let (_, current_frame) = self.frame_stack.last_mut().unwrap();

                            // Call the host function with the current stack
                            let result = host_func(&mut current_frame.stack)
                                .map_err(ExecError::ExecutionFault)?;

                            // Host functions typically return ProgramEnd, but we should handle other continuations
                            match result {
                                Continuation::ProgramEnd => {
                                    // Host function completed, continue execution
                                    break;
                                }
                                other => {
                                    // Host function returned a different continuation - handle recursively
                                    continuation = other;
                                    continue;
                                }
                            }
                        }
                    }
                }
                break 'execute_current;
            }
        }
    }

    #[inline(always)]
    fn run_with_tick_budget(
        &mut self,
        remaining_ticks: Option<usize>,
    ) -> Result<RunResult, ExecError> {
        match remaining_ticks {
            Some(ticks) => self.run_core::<true>(ticks),
            None => self.run_core::<false>(0),
        }
    }

    /// Run until completion.
    pub fn run(&mut self) -> Result<(), ExecError> {
        let run_result = self.run_with_tick_budget(None);
        if run_result.is_err() {
            self.clear_and_recycle_active_frames();
        }
        match run_result? {
            RunResult::Complete => Ok(()),
            RunResult::Paused => Err(ExecError::ExecutionFault(Fault::OutOfTicks)),
        }
    }

    /// Run for up to `ticks` instruction ticks and return whether execution completed or paused.
    pub fn run_for_ticks(&mut self, ticks: usize) -> Result<RunResult, ExecError> {
        let run_result = self.run_with_tick_budget(Some(ticks));
        if run_result.is_err() {
            self.clear_and_recycle_active_frames();
        }
        run_result
    }
}

#[cfg(test)]
mod tests {
    use crate::exec::{ExecutionState, RunResult, Value};
    use crate::instance::mk_instance;
    use crate::module::Module;

    #[test]
    fn load_run_itoa_with_execution_state() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();
        let linked = mk_instance(module).unwrap();

        let mut exec_state = ExecutionState::new();

        // Add the module and run the itoa function
        exec_state.add_module(0, linked);
        exec_state.prepare(0, 1, &[Value::I32(123)]).unwrap();
        exec_state.run().unwrap();
    }

    #[test]
    fn test_execution_state_single_module() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();
        let linked = mk_instance(module).unwrap();

        let mut exec_state = ExecutionState::new();

        // Add the module to the execution state
        exec_state.add_module(0, linked);

        // Prepare and run a function
        exec_state.prepare(0, 1, &[Value::I32(123)]).unwrap();
        exec_state.run().unwrap();

        // Should have executed successfully
        assert_eq!(exec_state.frame_stack_len(), 0);
    }

    #[test]
    fn test_execution_state_module_management() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();
        let linked = mk_instance(module).unwrap();

        let mut exec_state = ExecutionState::new();

        // Test adding and retrieving modules
        exec_state.add_module(42, linked);
        assert!(exec_state.get_module(42).is_some());
        assert!(exec_state.get_module(99).is_none());

        // Test mutable access
        assert!(exec_state.get_module_mut(42).is_some());
        assert!(exec_state.get_module_mut(99).is_none());
    }

    #[test]
    fn test_execution_state_empty_run() {
        let mut exec_state = ExecutionState::new();

        // Running with no frames should return immediately
        let result = exec_state.run();
        assert!(result.is_ok());
        assert_eq!(exec_state.frame_stack_len(), 0);
    }

    #[test]
    fn test_execution_state_nonexistent_module() {
        let mut exec_state = ExecutionState::new();

        // Try to prepare a function in a non-existent module
        let result = exec_state.prepare(999, 0, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_for_ticks_pause_and_resume() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();
        let linked = mk_instance(module).unwrap();

        let mut exec_state = ExecutionState::new();
        exec_state.add_module(0, linked);
        exec_state.prepare(0, 1, &[Value::I32(123)]).unwrap();

        // Zero budget should pause without consuming state.
        assert_eq!(exec_state.run_for_ticks(0).unwrap(), RunResult::Paused);
        assert!(exec_state.frame_stack_len() > 0);

        // Resume in small slices until completion.
        let mut completed = false;
        for _ in 0..10_000 {
            match exec_state.run_for_ticks(64).unwrap() {
                RunResult::Paused => continue,
                RunResult::Complete => {
                    completed = true;
                    break;
                }
            }
        }
        assert!(
            completed,
            "execution did not complete within expected slices"
        );
        assert_eq!(exec_state.frame_stack_len(), 0);
    }

    #[test]
    fn test_run_for_ticks_empty_execution_is_complete() {
        let mut exec_state = ExecutionState::new();
        assert_eq!(exec_state.run_for_ticks(0).unwrap(), RunResult::Complete);
    }

    // Note: Multi-module tests would require creating multiple WASM modules
    // with imports/exports which is more complex. For now, we test the
    // infrastructure with single modules.
}
