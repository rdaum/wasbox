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
use crate::frame::Frame;
use crate::import::{HostFunction, ImportResolver, ImportedFunction};
use crate::instance::{LinkError, TableInstance, WASM_PAGE_SIZE};
use crate::memory::Memory;
use crate::memory::SliceMemory;
use crate::module::Module;
use crate::module::{Global, ReferenceType};
use crate::op::{MemArg, Op};
use crate::stack::Stack;
use crate::{FuncType, Instance, Type, TypeSignature, ValueType};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

/// How many ticks we allow before we stop execution when running expressions during the link
/// phase (Active data expressions etc)
const EXPR_TICK_LIMIT: usize = 1 << 10;
/// Default tick limit for normal program execution.
const EXEC_TICK_LIMIT: usize = 1 << 24;

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
    UnexpectedResult(Continuation),
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

fn resolve_type(types: &[FuncType], ts: TypeSignature) -> Result<Type, Fault> {
    match ts {
        TypeSignature::ValueType(v) => Ok(Type::ValueType(v)),
        TypeSignature::Index(idx) => {
            let ft = types
                .get(idx as usize)
                .ok_or(Fault::UnresolvableTypeIndex(idx))
                .cloned();
            Ok(Type::FunctionType(ft?))
        }
    }
}

/// Unified branch execution using structured control flow
fn execute_branch(frame: &mut Frame, depth: usize) -> Result<(), Fault> {
    if depth >= frame.control_stack.len() {
        return Err(Fault::ControlStackUnderflow);
    }

    // Find the target control block using WASM branch semantics
    // All scopes are valid branch targets: Block, Loop, Function, and IfElse
    let mut target_idx = None;

    for (branch_target_count, (i, _control)) in
        frame.control_stack.iter().enumerate().rev().enumerate()
    {
        // All control scopes are valid branch targets
        if branch_target_count == depth {
            target_idx = Some(i);
            break;
        }
    }

    let target_idx = target_idx.ok_or(Fault::ControlStackUnderflow)?;
    let target_scope_type = frame.control_stack[target_idx].scope_type;
    let pop_depth = match target_scope_type {
        ScopeType::Loop => frame.control_stack.len() - 1 - target_idx, // Don't pop the loop
        _ => frame.control_stack.len() - target_idx, // Pop the target block/function too
    };

    // Get the target control block's signature to know what values it expects
    let target_signature = frame.control_stack[target_idx].signature.clone();
    let target_stack_width = frame.control_stack[target_idx].stack_width;

    // Pop the branch values from the stack (these will be provided to the target)
    let branch_values = match &target_signature {
        Type::ValueType(vt) => {
            if *vt != ValueType::Unit && frame.stack.width() > 0 {
                vec![Value::pop_from(*vt, &mut frame.stack)?]
            } else {
                vec![]
            }
        }
        Type::FunctionType(ft) => {
            // Pre-allocate and assign by index to avoid double-reverse
            let mut branch_values = vec![Value::Unit; ft.results.len()];
            for (i, vt) in ft.results.iter().enumerate().rev() {
                branch_values[i] = Value::pop_from(*vt, &mut frame.stack)?;
            }
            branch_values
        }
    };

    // Pop all the control blocks up to (but not including) the target
    for _ in 0..pop_depth {
        let _c = frame
            .control_stack
            .pop()
            .ok_or(Fault::ControlStackUnderflow)?;
    }

    // Shrink stack to the target block's width
    frame.stack.shrink_to(target_stack_width);

    // Now provide the branch values to the target block
    for value in branch_values {
        value.push_to(&mut frame.stack);
    }

    // For structured control flow, we need to find where to jump based on scope type
    match target_scope_type {
        ScopeType::Function => {
            // Branching to function scope means returning from the function
            frame.pc = frame.program.ops.len(); // This will cause the main loop to exit
        }
        ScopeType::Loop => {
            // For loops, branch to the beginning of the loop (where StartScope is)
            // Scan backward to find the corresponding StartScope
            let mut loop_depth = 0;
            let mut target_pc = frame.pc;

            for i in (0..frame.pc).rev() {
                match &frame.program.ops[i] {
                    Op::EndScope(ScopeType::Loop) => loop_depth += 1,
                    Op::StartScope(_, ScopeType::Loop) if loop_depth == 0 => {
                        target_pc = i + 1; // Jump to instruction after StartScope
                        break;
                    }
                    Op::StartScope(_, ScopeType::Loop) => loop_depth -= 1,
                    _ => {}
                }
            }
            frame.pc = target_pc;
        }
        _ => {
            // For Block, IfElse, Function: branch to the end (after EndScope)
            // Since we've popped the target scope too, we need to find its EndScope
            // The scope depth should account for the fact that we popped all intervening scopes AND the target
            let mut scope_depth = pop_depth - 1; // -1 because we also popped the target

            while frame.pc < frame.program.ops.len() {
                match &frame.program.ops[frame.pc] {
                    Op::StartScope(_, _) => scope_depth += 1,
                    Op::EndScope(_) => {
                        if scope_depth == 0 {
                            frame.pc += 1; // Jump past the EndScope
                            break;
                        }
                        scope_depth -= 1;
                    }
                    _ => {}
                }
                frame.pc += 1;
            }
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

fn validate_range(start: u32, len: u32, total_len: usize) -> Result<(usize, usize), Fault> {
    let start = start as usize;
    let len = len as usize;
    let end = start.checked_add(len).ok_or(Fault::MemoryOutOfBounds)?;
    if end > total_len {
        return Err(Fault::MemoryOutOfBounds);
    }
    Ok((start, end))
}

fn execute(
    frame: &mut Frame,
    memories: &mut [&mut dyn Memory],
    globals: &mut [GlobalVar],
    tables: &mut [TableInstance],
    element_segments: &mut [Option<Vec<Value>>],
    max_ticks: usize,
    types: &[FuncType],
    functions: &[usize],
    current_module_id: u32,
) -> Result<(Continuation, usize), Fault> {
    let mut ticks_used = 0;
    loop {
        // Pull next opcode from the program
        let pc = frame.pc;
        if pc >= frame.program.ops.len() {
            // We've reached the end of the program
            return Ok((Continuation::ProgramEnd, ticks_used));
        }
        ticks_used += 1;
        if ticks_used >= max_ticks {
            return Err(Fault::OutOfTicks);
        }
        frame.pc += 1;
        let op = frame.program.ops[pc].clone();

        match op {
            Op::Nop => {}
            Op::Unreachable => {
                return Err(Fault::Unreachable);
            }
            Op::StartScope(sig, scope_type) => {
                let resolved_type = resolve_type(types, sig)?;
                frame.push_control(resolved_type, scope_type);
            }
            Op::EndScope(c) => {
                // If this is EndScope(Program), we need to preserve the stack for return value.
                if let ScopeType::Program = &c {
                    return Ok((Continuation::DoneReturn, ticks_used));
                }
                let (end_scope, result_values) = frame.pop_control()?;

                // Shrink-stack to the width declared in the control scope.
                frame.stack.shrink_to(end_scope.stack_width);
                for value in result_values {
                    value.push_to(&mut frame.stack);
                }
            }
            Op::If => {
                // Pop condition from stack, evaluate.
                let condition = frame.stack.pop_u32()?;
                if condition == 0 {
                    // Skip to else block or end of if - scan forward to find it
                    let mut depth = 0;
                    let _start_pc = frame.pc;

                    while frame.pc < frame.program.ops.len() {
                        match &frame.program.ops[frame.pc] {
                            Op::StartScope(_, ScopeType::IfElse) => depth += 1,
                            Op::Else if depth == 0 => {
                                frame.pc += 1; // Move past the Else op
                                break;
                            }
                            Op::EndScope(ScopeType::IfElse) if depth == 0 => {
                                break; // No else block, go to end
                            }
                            Op::EndScope(ScopeType::IfElse) => depth -= 1,
                            _ => {}
                        }
                        frame.pc += 1;
                    }
                } else {
                    // Continue to then block (next instruction)
                }
            }
            Op::Else => {
                // Skip to end of if block - scan forward to find matching EndScope
                let mut depth = 0;

                while frame.pc < frame.program.ops.len() {
                    match &frame.program.ops[frame.pc] {
                        Op::StartScope(_, ScopeType::IfElse) => depth += 1,
                        Op::EndScope(ScopeType::IfElse) if depth == 0 => {
                            break; // Found the end of this if block
                        }
                        Op::EndScope(ScopeType::IfElse) => depth -= 1,
                        _ => {}
                    }
                    frame.pc += 1;
                }
            }
            Op::Br(depth) => {
                execute_branch(frame, depth as usize)?;
                continue;
            }
            Op::BrIf(depth) => {
                let condition = frame.stack.pop_u32()?;
                if condition != 0 {
                    execute_branch(frame, depth as usize)?;
                    continue;
                }
            }
            Op::BrTable(table, default) => {
                let index = frame.stack.pop_u32()? as usize;
                let depth = if index < table.len() {
                    table[index]
                } else {
                    default
                } as usize;

                execute_branch(frame, depth)?;
                continue;
            }
            Op::Return => {
                // Return immediately from function - don't pop control blocks, just exit
                // The stack should contain the return value(s) for the function
                return Ok((Continuation::DoneReturn, ticks_used));
            }
            Op::Call(c) => {
                return Ok((Continuation::Call(c), ticks_used));
            }
            Op::CallIndirect(_type_idx, table_idx) => {
                // Pop the table index from the stack (the actual index to use)
                let table_index = frame.stack.pop_u32()?;

                // Look up the function reference in the specified table
                if table_idx as usize >= tables.len() {
                    return Err(Fault::UndefinedElement); // Table index out of bounds
                }
                let table = &tables[table_idx as usize];
                let table_elements = table.elements.borrow();

                if table_index as usize >= table_elements.len() {
                    return Err(Fault::UndefinedElement); // Table index out of bounds
                }

                return match &table_elements[table_index as usize] {
                    None => Err(Fault::UninitializedElement), // Uninitialized table element
                    Some(Value::FuncRef(Some(func_ref))) => {
                        // Type check: verify the function signature matches the expected type.
                        if _type_idx as usize >= types.len() {
                            return Err(Fault::UnresolvableTypeIndex(_type_idx));
                        }
                        let expected_type = &types[_type_idx as usize];

                        // For local module references we can type-check immediately. For external
                        // references we defer type-checking to the cross-module dispatch path.
                        let target_module_id = func_ref.module_id;
                        if target_module_id == current_module_id {
                            if (func_ref.func_idx as usize) >= functions.len() {
                                return Err(Fault::UnresolvableTypeIndex(func_ref.func_idx));
                            }
                            let actual_type_idx = functions[func_ref.func_idx as usize];
                            if actual_type_idx >= types.len() {
                                return Err(Fault::UnresolvableTypeIndex(actual_type_idx as u32));
                            }
                            let actual_type = &types[actual_type_idx];
                            if expected_type.params != actual_type.params
                                || expected_type.results != actual_type.results
                            {
                                return Err(Fault::IndirectCallTypeMismatch);
                            }
                            Ok((Continuation::Call(func_ref.func_idx), ticks_used))
                        } else {
                            Ok((
                                Continuation::CallExternal {
                                    module_id: target_module_id,
                                    func_id: func_ref.func_idx,
                                    expected_type: Some((current_module_id, _type_idx)),
                                },
                                ticks_used,
                            ))
                        }
                    }
                    Some(Value::FuncRef(None)) => Err(Fault::UninitializedElement), // Null function reference
                    _ => Err(Fault::UndefinedElement), // Invalid table element
                };
            }
            Op::Drop => {
                frame.stack.pop_u64()?;
            }
            Op::Select => {
                //The select instruction returns its first operand if $condition is true, or its second operand otherwise.
                let condition = frame.stack.pop_i32()?;
                let val2 = frame.stack.pop_u64()?; // Second operand (popped first)
                let val1 = frame.stack.pop_u64()?; // First operand (popped second)
                if condition != 0 {
                    frame.stack.push_u64(val1); // Return first operand if condition is true
                } else {
                    frame.stack.push_u64(val2); // Return second operand if condition is false
                }
            }
            Op::GetLocal(idx) => {
                frame.push_local_to_stack(idx)?;
            }
            Op::SetLocal(idx) => {
                frame.set_local_from_stack(idx, true)?;
            }
            Op::TeeLocal(idx) => {
                frame.set_local_from_stack(idx, false)?;
            }
            Op::GetGlobal(g) => {
                if g as usize >= globals.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let gv = &globals[g as usize];
                gv.value.push_to(&mut frame.stack);
            }
            Op::SetGlobal(g) => {
                if g as usize >= globals.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let value = Value::pop_from(globals[g as usize].decl.ty, &mut frame.stack)?;
                globals[g as usize].value = value;
            }
            Op::TableGet(table_idx) => {
                let idx = frame.stack.pop_u32()?;
                if table_idx as usize >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let table = &tables[table_idx as usize];
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
                if table_idx as usize >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                let table = &mut tables[table_idx as usize];
                let value = pop_table_value(&mut frame.stack, table.ref_type)?;

                let idx = frame.stack.pop_u32()?;
                let mut table_elements = table.elements.borrow_mut();
                if idx as usize >= table_elements.len() {
                    return Err(Fault::MemoryOutOfBounds);
                }
                table_elements[idx as usize] = Some(value);
            }
            Op::TableInit(table_idx, elem_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let table = tables
                    .get_mut(table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let segment = element_segments
                    .get(elem_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let segment_values = segment.as_deref().unwrap_or(&[]);

                let (src_start, src_end) = validate_range(src, len, segment_values.len())?;
                let mut table_elements = table.elements.borrow_mut();
                let (dst_start, _) = validate_range(dst, len, table_elements.len())?;
                for (i, value) in segment_values[src_start..src_end].iter().enumerate() {
                    table_elements[dst_start + i] = Some(*value);
                }
            }
            Op::ElemDrop(elem_idx) => {
                let segment = element_segments
                    .get_mut(elem_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                *segment = None;
            }
            Op::TableCopy(dst_table_idx, src_table_idx) => {
                let len = frame.stack.pop_u32()?;
                let src = frame.stack.pop_u32()?;
                let dst = frame.stack.pop_u32()?;

                let src_table_idx = src_table_idx as usize;
                let dst_table_idx = dst_table_idx as usize;
                if src_table_idx >= tables.len() || dst_table_idx >= tables.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }

                if tables[src_table_idx].ref_type != tables[dst_table_idx].ref_type {
                    return Err(Fault::InvalidRefType);
                }

                if src_table_idx == dst_table_idx {
                    let mut table_elements = tables[src_table_idx].elements.borrow_mut();
                    let (src_start, src_end) = validate_range(src, len, table_elements.len())?;
                    let (dst_start, _) = validate_range(dst, len, table_elements.len())?;
                    let copied = table_elements[src_start..src_end].to_vec();
                    for (i, value) in copied.into_iter().enumerate() {
                        table_elements[dst_start + i] = value;
                    }
                } else {
                    let copied = {
                        let src_elements = tables[src_table_idx].elements.borrow();
                        let (src_start, src_end) = validate_range(src, len, src_elements.len())?;
                        src_elements[src_start..src_end].to_vec()
                    };
                    let mut dst_elements = tables[dst_table_idx].elements.borrow_mut();
                    let (dst_start, _) = validate_range(dst, len, dst_elements.len())?;
                    for (i, value) in copied.into_iter().enumerate() {
                        dst_elements[dst_start + i] = value;
                    }
                }
            }
            Op::TableGrow(table_idx) => {
                let delta = frame.stack.pop_u32()?;
                let table = tables
                    .get_mut(table_idx as usize)
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
                frame.stack.push_u32(old_len as u32);
            }
            Op::TableSize(table_idx) => {
                let table = tables
                    .get(table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let table_len = table.elements.borrow().len();
                frame.stack.push_u32(table_len as u32);
            }
            Op::TableFill(table_idx) => {
                let len = frame.stack.pop_u32()?;
                let table = tables
                    .get_mut(table_idx as usize)
                    .ok_or(Fault::GlobalIndexOutOfBounds)?;
                let value = pop_table_value(&mut frame.stack, table.ref_type)?;
                let dst = frame.stack.pop_u32()?;

                let mut table_elements = table.elements.borrow_mut();
                let (dst_start, dst_end) = validate_range(dst, len, table_elements.len())?;
                for slot in &mut table_elements[dst_start..dst_end] {
                    *slot = Some(value);
                }
            }
            Op::LoadI32(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_i32(addr)?;
                frame.stack.push_i32(value);
            }
            Op::LoadI64(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_i64(addr)?;
                frame.stack.push_i64(value);
            }
            Op::LoadF32(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_f32(addr)?;
                frame.stack.push_f32(value);
            }
            Op::LoadF64(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_f64(addr)?;
                frame.stack.push_f64(value);
            }

            // Extending load, signed
            Op::Load8SE(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u8(addr)? as i8 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load16Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u16(addr)? as i16 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load8I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u8(addr)? as i8 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load16I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u16(addr)? as i16 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load32I64Se(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u32(addr)? as i32 as i64;
                frame.stack.push_i64(value);
            }

            // Extending load, unsigned
            Op::Load8Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u8(addr)? as u32;
                frame.stack.push_u32(value);
            }
            Op::Load16Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u16(addr)? as u32;
                frame.stack.push_u32(value);
            }
            Op::Load8I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u8(addr)? as u64;
                frame.stack.push_u64(value);
            }
            Op::Load16I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u16(addr)? as u64;
                frame.stack.push_u64(value);
            }
            Op::Load32I64Ze(addr) => {
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let value = memory.get_u32(addr)? as u64;
                frame.stack.push_u64(value);
            }
            Op::StoreI32(addr) => {
                let value = frame.stack.pop_i32()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_i32(addr, value)?;
            }
            Op::StoreI64(addr) => {
                let value = frame.stack.pop_i64()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_i64(addr, value)?;
            }
            Op::StoreF32(addr) => {
                let value = frame.stack.pop_f32()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_f32(addr, value)?;
            }
            Op::StoreF64(addr) => {
                let value = frame.stack.pop_f64()?;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_f64(addr, value)?;
            }

            // Silently narrow the width of the value
            Op::Store8_32(addr) => {
                let value = frame.stack.pop_i32()? as u8;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_u8(addr, value)?;
            }
            Op::Store16_32(addr) => {
                let value = frame.stack.pop_i32()? as u16;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_u16(addr, value)?;
            }
            Op::Store8_64(addr) => {
                let value = frame.stack.pop_i64()? as u8;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_u8(addr, value)?;
            }
            Op::Store16_64(addr) => {
                let value = frame.stack.pop_i64()? as u16;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_u16(addr, value)?;
            }
            Op::Store32_64(addr) => {
                let value = frame.stack.pop_i64()? as u32;
                let memarg = addr;
                let addr = adjust_memarg(&mut frame.stack, &memarg)?;
                let memory = memories
                    .get_mut(memarg.memory as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                memory.set_u32(addr, value)?;
            }

            Op::I32Const(v) => {
                frame.stack.push_i32(v);
            }
            Op::I64Const(v) => {
                frame.stack.push_i64(v);
            }
            Op::F32Const(v) => {
                frame.stack.push_u32(v.to_bits());
            }
            Op::F64Const(v) => {
                frame.stack.push_u64(v.to_bits());
            }
            Op::MemorySize(mem_idx) => {
                let memory = memories
                    .get_mut(mem_idx as usize)
                    .ok_or(Fault::MemoryOutOfBounds)?;
                let size_in_bytes = memory.size();
                let size_in_pages = size_in_bytes / WASM_PAGE_SIZE;
                frame.stack.push_u32(size_in_pages as u32);
            }
            Op::MemoryGrow(mem_idx) => {
                let delta = frame.stack.pop_i32()?;
                let memory = memories
                    .get_mut(mem_idx as usize)
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
                    match memory.grow(new_size) {
                        Ok(_) => frame.stack.push_i32(old_page_count as i32),
                        Err(_) => frame.stack.push_i32(-1),
                    }
                }
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
                if (func_index as usize) >= functions.len() {
                    return Err(Fault::GlobalIndexOutOfBounds);
                }
                frame.stack.push_ref(Some(encode_func_ref(FuncRef {
                    module_id: current_module_id,
                    func_idx: func_index,
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
            Op::SelectT(ref _types) => {
                // For now, implement same as regular select
                // TODO: Add type validation
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

fn adjust_memarg(stack: &mut Stack, memarg: &MemArg) -> Result<usize, Fault> {
    let base_addr = stack.pop_i32()? as usize;

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
    V128(u128),
    FuncRef(Option<FuncRef>),
    ExternRef(Option<u32>),
    Unit,
}

impl Eq for Value {}

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
    pub(crate) fn type_of(&self) -> ValueType {
        match self {
            Value::I32(_) => ValueType::I32,
            Value::I64(_) => ValueType::I64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::V128(_) => ValueType::V128,
            Value::FuncRef(_) => ValueType::FuncRef,
            Value::ExternRef(_) => ValueType::ExternRef,
            Value::Unit => ValueType::Unit,
        }
    }

    pub fn pop_from(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => {
                stack.pop_u64()?;
                Value::Unit
            }
            ValueType::I32 => Value::I32(stack.pop_i32()?),
            ValueType::I64 => Value::I64(stack.pop_i64()?),
            ValueType::F32 => Value::F32(stack.pop_f32()?),
            ValueType::F64 => Value::F64(stack.pop_f64()?),
            ValueType::V128 => {
                let (l, r) = (stack.pop_u64()?, stack.pop_u64()?);
                Value::V128((r as u128) << 64 | l as u128)
            }
            ValueType::FuncRef => Value::FuncRef(stack.pop_ref()?.map(decode_func_ref)),
            ValueType::ExternRef => Value::ExternRef(stack.pop_ref()?.map(|v| v as u32)),
        })
    }

    pub fn top_of(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => {
                stack.pop_u64()?;
                Value::Unit
            }
            ValueType::I32 => Value::I32(stack.top_i32()?),
            ValueType::I64 => Value::I64(stack.top_i64()?),
            ValueType::F32 => Value::F32(stack.top_f32()?),
            ValueType::F64 => Value::F64(stack.top_f64()?),
            ValueType::V128 => {
                let (l, r) = (stack.top_u64()?, stack.top_u64()?);
                Value::V128((r as u128) << 64 | l as u128)
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

    pub fn push_to(&self, stack: &mut Stack) {
        match self {
            Value::I32(v) => stack.push_i32(*v),
            Value::I64(v) => stack.push_i64(*v),
            Value::F32(v) => stack.push_f32(*v),
            Value::F64(v) => stack.push_f64(*v),
            Value::V128(v) => {
                stack.push_u64(*v as u64);
                stack.push_u64((*v >> 64) as u64);
            }
            Value::FuncRef(v) => stack.push_ref(v.map(encode_func_ref)),
            Value::ExternRef(v) => stack.push_ref(v.map(|v| v as u64)),
            Value::Unit => {
                stack.push_u64(0);
            }
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
    let const_program = decode(&expression).unwrap();
    let return_types = vec![return_type];
    let mut global_exec_frame = Frame {
        locals: vec![Value::Unit; 0],
        program: const_program,
        stack: Stack::new(),
        pc: 0,
        control_stack: vec![],
        return_types,
    };
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
    let result = execute(
        &mut global_exec_frame,
        &mut const_prg_memories,
        &mut const_prg_globals,
        &mut const_prg_tables,
        &mut const_prg_element_segments,
        EXPR_TICK_LIMIT,
        types,
        functions,
        current_module_owner_id,
    )?;
    let (result, _) = result;
    // Must be `ProgramEnd`, or there's a bug, and that's UnexpectedResult
    match result {
        Continuation::ProgramEnd | Continuation::DoneReturn => {}
        _ => return Err(Fault::UnexpectedResult(result)),
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

fn build_function_type_mapping(instance: &Instance) -> Vec<usize> {
    let mut function_type_mapping = Vec::new();

    // Add imported functions first.
    for (_, _, import) in &instance.module.imports {
        if let crate::module::Import::Func(type_idx) = import {
            function_type_mapping.push(*type_idx as usize);
        }
    }

    // Then add local functions.
    function_type_mapping.extend(instance.module.functions.iter().copied());
    function_type_mapping
}

fn execute_module_frame(
    module_id: u32,
    frame: &mut Frame,
    instance: &mut Instance,
    max_ticks: usize,
) -> Result<(Continuation, usize), Fault> {
    let function_type_mapping = build_function_type_mapping(instance);
    let (memories, globals, tables, element_segments, module) = (
        &mut instance.memories,
        &mut instance.globals,
        &mut instance.tables,
        &mut instance.element_segments,
        &instance.module,
    );
    let mut memory_refs: Vec<&mut dyn Memory> =
        memories.iter_mut().map(|m| m as &mut dyn Memory).collect();

    // Memory instructions are validated at module level, but keep an empty fallback memory so
    // malformed modules trap cleanly instead of panicking.
    if memory_refs.is_empty() {
        let mut fallback_memory = crate::VectorMemory::new(0, None);
        let mut fallback_memories: [&mut dyn Memory; 1] = [&mut fallback_memory];
        execute(
            frame,
            &mut fallback_memories,
            globals,
            tables,
            element_segments,
            max_ticks,
            &module.types,
            &function_type_mapping,
            module_id,
        )
    } else {
        execute(
            frame,
            &mut memory_refs,
            globals,
            tables,
            element_segments,
            max_ticks,
            &module.types,
            &function_type_mapping,
            module_id,
        )
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
    /// Final result when all execution completes
    result: Option<Vec<Value>>,
}

impl ExecutionState {
    pub fn new() -> Self {
        ExecutionState {
            modules: HashMap::new(),
            module_owner_map: HashMap::new(),
            frame_stack: vec![],
            result: None,
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
        let instance = self.modules.remove(&module_id)?;
        self.module_owner_map.remove(&instance.owner_id);
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
        let instance = self
            .modules
            .get(&module_id)
            .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;

        // Allow invoking exported imported functions by creating a trampoline frame.
        let num_imported_funcs = instance.imported_functions.len() as u32;
        if funcidx < num_imported_funcs {
            let function_type_mapping = build_function_type_mapping(instance);
            let type_idx =
                *function_type_mapping
                    .get(funcidx as usize)
                    .ok_or(ExecError::ExecutionFault(Fault::UnresolvableTypeIndex(
                        funcidx,
                    )))?;
            let func_type = &instance.module.types[type_idx];

            for (i, (expected, actual)) in func_type.params.iter().zip(args.iter()).enumerate() {
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

            let mut frame = Frame {
                locals: vec![],
                return_types: program.return_types.clone(),
                program,
                stack: Stack::new(),
                pc: 0,
                control_stack: vec![],
            };
            for arg in args {
                arg.push_to(&mut frame.stack);
            }
            self.frame_stack.push((module_id, frame));
            return Ok(());
        }

        let frame = instance
            .frame_for_funcidx(funcidx, args)
            .map_err(ExecError::LinkageError)?;

        self.frame_stack.push((module_id, frame));
        Ok(())
    }

    fn run_with_tick_budget(
        &mut self,
        mut remaining_ticks: Option<usize>,
    ) -> Result<RunResult, ExecError> {
        loop {
            if self.frame_stack.is_empty() {
                return Ok(RunResult::Complete);
            }
            if let Some(remaining) = remaining_ticks {
                if remaining == 0 {
                    return Ok(RunResult::Paused);
                }
            }

            let (current_module_id, _) = self.frame_stack.last().unwrap();
            let current_module_id = *current_module_id;
            let max_ticks = remaining_ticks
                .map(|remaining| remaining.saturating_add(1))
                .unwrap_or(EXEC_TICK_LIMIT);

            // Execute current frame
            let result = {
                let (_, frame) = self.frame_stack.last_mut().unwrap();
                let module = self.modules.get_mut(&current_module_id).unwrap();
                execute_module_frame(current_module_id, frame, module, max_ticks)
            };

            if let Some(remaining) = remaining_ticks.as_mut() {
                match result {
                    Ok((_, ticks_used)) => {
                        *remaining = remaining.saturating_sub(ticks_used);
                    }
                    Err(Fault::OutOfTicks) => {
                        return Ok(RunResult::Paused);
                    }
                    Err(_) => {}
                }
            }

            let mut continuation = result.map(|(continuation, _)| continuation);

            // Handle continuations in a loop to support recursive calls
            loop {
                match continuation {
                    Ok(Continuation::ProgramEnd) | Ok(Continuation::DoneReturn) => {
                        // Handle function return
                        let (_, completed_frame) = self.frame_stack.pop().unwrap();

                        // Collect return values
                        let mut return_values = vec![
                            (ValueType::Unit, Value::Unit);
                            completed_frame.return_types.len()
                        ];
                        let mut temp_stack = completed_frame.stack;
                        for (i, rt) in completed_frame.return_types.iter().enumerate().rev() {
                            let value = Value::pop_from(*rt, &mut temp_stack)
                                .map_err(ExecError::ExecutionFault)?;
                            return_values[i] = (*rt, value);
                        }

                        if let Some((_, caller_frame)) = self.frame_stack.last_mut() {
                            // Push return values to caller's stack
                            for (_, v) in return_values {
                                v.push_to(&mut caller_frame.stack);
                            }
                        } else {
                            // No caller, this is the final result
                            self.result = Some(return_values.into_iter().map(|(_, v)| v).collect());
                            return Ok(RunResult::Complete);
                        }
                        break; // Exit continuation loop to continue main execution loop
                    }
                    Ok(Continuation::Call(funcidx)) => {
                        // Function call - check if it's imported or local
                        let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                        let module = self.modules.get(&current_module_id).unwrap();

                        // Check if this is an imported function
                        let num_imported_funcs = module.imported_functions.len() as u32;
                        if funcidx < num_imported_funcs {
                            // This is an imported function - dispatch via continuation
                            let imported_func = &module.imported_functions[funcidx as usize];
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
                                            continuation = Ok(other);
                                            continue;
                                        }
                                    }
                                }
                                ImportedFunction::Module {
                                    module_id,
                                    func_idx,
                                } => {
                                    // Inter-module call
                                    continuation = Ok(Continuation::CallExternal {
                                        module_id: *module_id,
                                        func_id: *func_idx,
                                        expected_type: None,
                                    });
                                    continue;
                                }
                            }
                        } else {
                            // Local function call
                            let local_func_idx = funcidx - num_imported_funcs;
                            let func_index = local_func_idx as usize;
                            if func_index >= module.module.functions.len() {
                                return Err(ExecError::ExecutionFault(
                                    Fault::GlobalIndexOutOfBounds,
                                ));
                            }

                            let type_idx = module.module.functions[func_index];
                            let func_type = &module.module.types[type_idx];

                            let mut args = vec![Value::Unit; func_type.params.len()];
                            for (i, param_type) in func_type.params.iter().enumerate().rev() {
                                let value = Value::pop_from(*param_type, &mut current_frame.stack)
                                    .map_err(ExecError::ExecutionFault)?;
                                args[i] = value;
                            }

                            // Create new frame for the called function
                            let frame = module
                                .frame_for_funcidx(funcidx, &args)
                                .map_err(ExecError::LinkageError)?;
                            self.frame_stack.push((current_module_id, frame));
                            break; // Exit continuation loop to continue main execution loop
                        }
                    }
                    Ok(Continuation::CallExternal {
                        module_id,
                        func_id,
                        expected_type,
                    }) => {
                        // Inter-module call
                        let target_module_key = self
                            .ensure_runtime_module_available(module_id)
                            .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
                        let (_, current_frame) = self.frame_stack.last_mut().unwrap();
                        let target_module = self
                            .modules
                            .get(&target_module_key)
                            .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;

                        // Get function signature from the target module.
                        let function_type_mapping = build_function_type_mapping(target_module);
                        let func_index = func_id as usize;
                        if func_index >= function_type_mapping.len() {
                            return Err(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds));
                        }

                        let type_idx = function_type_mapping[func_index];
                        let func_type = &target_module.module.types[type_idx];

                        if let Some((source_module_id, expected_type_idx)) = expected_type {
                            let source_module = self
                                .modules
                                .get(&source_module_id)
                                .ok_or(ExecError::ExecutionFault(Fault::GlobalIndexOutOfBounds))?;
                            let expected_type = source_module
                                .module
                                .types
                                .get(expected_type_idx as usize)
                                .ok_or(ExecError::ExecutionFault(Fault::UnresolvableTypeIndex(
                                    expected_type_idx,
                                )))?;
                            if expected_type.params != func_type.params
                                || expected_type.results != func_type.results
                            {
                                return Err(ExecError::ExecutionFault(
                                    Fault::IndirectCallTypeMismatch,
                                ));
                            }
                        }

                        // If the target is imported, dispatch recursively without creating a frame.
                        if func_id < target_module.imported_functions.len() as u32 {
                            let imported_func = &target_module.imported_functions[func_id as usize];
                            match imported_func {
                                ImportedFunction::Host(host_func) => {
                                    let result = host_func(&mut current_frame.stack)
                                        .map_err(ExecError::ExecutionFault)?;
                                    match result {
                                        Continuation::ProgramEnd => break,
                                        other => {
                                            continuation = Ok(other);
                                            continue;
                                        }
                                    }
                                }
                                ImportedFunction::Module {
                                    module_id,
                                    func_idx,
                                } => {
                                    continuation = Ok(Continuation::CallExternal {
                                        module_id: *module_id,
                                        func_id: *func_idx,
                                        expected_type: None,
                                    });
                                    continue;
                                }
                            }
                        } else {
                            // Create new frame in target module.
                            let mut args = vec![Value::Unit; func_type.params.len()];
                            for (i, param_type) in func_type.params.iter().enumerate().rev() {
                                let value = Value::pop_from(*param_type, &mut current_frame.stack)
                                    .map_err(ExecError::ExecutionFault)?;
                                args[i] = value;
                            }
                            let frame = target_module
                                .frame_for_funcidx(func_id, &args)
                                .map_err(ExecError::LinkageError)?;
                            self.frame_stack.push((target_module_key, frame));
                            break; // Exit continuation loop to continue main execution loop
                        }
                    }
                    Ok(Continuation::CallHost(host_func)) => {
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
                                continuation = Ok(other);
                                continue;
                            }
                        }
                    }
                    Err(fault) => {
                        // Clean up current frame on fault
                        self.frame_stack.pop();
                        return Err(ExecError::ExecutionFault(fault));
                    }
                }
            }
        }
    }

    /// Run until completion.
    pub fn run(&mut self) -> Result<(), ExecError> {
        let run_result = self.run_with_tick_budget(None);
        if run_result.is_err() {
            self.frame_stack.clear();
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
            self.frame_stack.clear();
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
