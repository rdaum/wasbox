// Copyright (C) 2024 Ryan Daum <ryan.daum@gmail.com>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
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
use crate::instance::{LinkError, WASM_PAGE_SIZE};
use crate::memory::Memory;
use crate::memory::SliceMemory;
use crate::module::Global;
use crate::op::{MemArg, Op};
use crate::stack::Stack;
use crate::Value::Unit;
use crate::{FuncType, Instance, Type, TypeSignature, ValueType};
use num_traits::Float;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

/// How many ticks we allow before we stop execution when running expressions during the link
/// phase (Active data expressions etc)
const EXPR_TICK_LIMIT: usize = 1 << 10;

#[derive(Debug)]
pub enum Continuation {
    Call(u32, Option<u32>),
    /// Program ran out of instructions
    ProgramEnd,
    /// An explicit return instruction was encountered.
    /// Stack should contain the return value.
    DoneReturn,
}

#[derive(Debug)]
pub enum Fault {
    /// Ran out of execution ticks
    OutOfTicks(usize, usize),
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
    /// Argument type mismatch
    ArgumentTypeMismatch(ValueType, ValueType),
    /// Call stack depth limit reached
    CallStackDepthLimit(usize, usize),
    /// Error decoding constant expression
    DecodeError,
}

impl Display for Fault {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::OutOfTicks(max_ticks, used_ticks) => {
                write!(f, "Out of ticks: {} > {}", used_ticks, max_ticks)
            }
            Fault::UnexpectedResult(c) => write!(f, "Unexpected result: {:?}", c),
            Fault::StackUnderflow => write!(f, "Stack underflow"),
            Fault::ControlStackUnderflow => write!(f, "Control stack underflow"),
            Fault::LocalIndexOutOfBounds => write!(f, "Local index out of bounds"),
            Fault::GlobalIndexOutOfBounds => write!(f, "Global index out of bounds"),
            Fault::MemoryOutOfBounds => write!(f, "Memory out of bounds"),
            Fault::CannotGrowMemory => write!(f, "Cannot grow memory"),
            Fault::UnresolvableTypeIndex(idx) => write!(f, "Unresolvable type index: {}", idx),
            Fault::ArgumentTypeMismatch(a, b) => {
                write!(f, "Argument type mismatch: expected {:?}, got {:?}", a, b)
            }
            Fault::CallStackDepthLimit(limit, value) => {
                write!(f, "Call stack depth limit reached: {} > {}", value, limit)
            }
            Fault::DecodeError => write!(f, "Error decoding constant expression"),
        }
    }
}

impl Error for Fault {}

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

// WASM treats NaN differently than Rust for min/max, so we need to handle it
// separately. Which sucks because this means a brand/comparison for every min/max.

fn min_nan_correct<F: Float>(a: F, b: F) -> F {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else {
        a.min(b)
    }
}

fn max_nan_correct<F: Float>(a: F, b: F) -> F {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else {
        a.max(b)
    }
}

fn execute<M>(
    frame: &mut Frame,
    memory: &mut M,
    globals: &mut [GlobalVar],
    max_ticks: usize,
    types: &[FuncType],
    ticks_used: &mut usize,
) -> Result<Continuation, Fault>
where
    M: Memory,
{
    loop {
        // Pull next opcode from the program
        let pc = frame.pc;
        if pc >= frame.program.ops.len() {
            // We've reached the end of the program
            return Ok(Continuation::ProgramEnd);
        }
        *ticks_used += 1;
        if *ticks_used >= max_ticks {
            return Err(Fault::OutOfTicks(max_ticks, *ticks_used));
        }
        frame.pc += 1;
        let op = frame.program.ops[pc].clone();

        match op {
            Op::Nop => {}
            Op::StartScope(sig, scope_type, label) => {
                let resolved_type = resolve_type(types, sig)?;
                frame.push_control(resolved_type, scope_type, label);
            }
            Op::EndScope(c) => {
                // If this is EndScope(Program), we need to preserve the stack for return value.
                if let ScopeType::Program = &c {
                    return Ok(Continuation::DoneReturn);
                }
                frame.pop_control()?;
            }
            Op::If(else_label) => {
                // Pop condition from stack, evaluate.
                // Then attempt to jump to else_label if false. If that fails, jump to end_label.
                let condition = frame.stack.pop_untyped()?;
                if condition == 0 {
                    assert!(frame.jump_label(else_label));
                }
                // Otherwise, we continue.
            }
            Op::Else(end_label) => {
                // Jump to end_label
                assert!(frame.jump_label(end_label));
            }
            Op::Br(label) => {
                // Pop until we hit the label, then jump to it.
                let label = loop {
                    let c = frame.pop_control()?;
                    if c.label == label {
                        break label;
                    }
                };
                assert!(frame.jump_label(label));
            }
            Op::BrIf(label) => {
                let condition = frame.stack.pop_untyped()?;
                // Walk back up the scope until we hit this label, and truncate back to that.
                if condition != 0 {
                    let pop_depth = frame
                        .control_stack
                        .iter()
                        .rev()
                        .position(|c| c.label == label)
                        .unwrap();
                    if pop_depth != 0 {
                        for _ in 0..pop_depth - 1 {
                            frame.pop_control()?;
                        }
                    }

                    assert!(frame.jump_label(label));
                }
            }
            Op::BrTable(table, default) => {
                let index = frame.stack.pop_untyped()? as usize;
                let label = if index < table.len() {
                    table[index]
                } else {
                    default
                };
                assert!(frame.jump_label(label));
            }
            Op::Return => {
                // Pop all control stack, and exit.
                while !frame.control_stack.is_empty() {
                    frame.pop_control()?;
                }
                // Stack should contain the return value of the function, in the type of the function,
                // which the caller knows, we don't make any assumptions about it.
                return Ok(Continuation::DoneReturn);
            }
            Op::Call(c) => {
                return Ok(Continuation::Call(c, None));
            }
            Op::CallIndirect(typesig) => {
                // Dump the stack for debuggy debuggy
                // In indirect, we pop the function index from the stack.
                let func_index = frame.stack.pop_untyped()?;
                return Ok(Continuation::Call(func_index as u32, Some(typesig)));
            }
            Op::Drop => {
                frame.stack.pop_untyped()?;
            }
            Op::Select => {
                //The select instruction returns its first operand if $condition is true, or its second operand otherwise.
                let condition = frame.stack.pop_untyped()?;
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                if condition != 0 {
                    frame.stack.push_i32(a);
                } else {
                    frame.stack.push_i32(b);
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
                let gv = &mut globals[g as usize];
                gv.value = Value::pop_from(gv.decl.ty, &mut frame.stack)?;
            }
            Op::LoadI32(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_i32(addr)?;
                frame.stack.push_i32(value);
            }
            Op::LoadI64(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_i64(addr)?;
                frame.stack.push_i64(value);
            }
            Op::LoadF32(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_f32(addr)?;
                frame.stack.push_f32(value);
            }
            Op::LoadF64(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_f64(addr)?;
                frame.stack.push_f64(value);
            }

            // Extending load, signed
            Op::Load8SE(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u8(addr)? as i8 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load16Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u16(addr)? as i16 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load8I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u8(addr)? as i8 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load16I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u16(addr)? as i16 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load32I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u32(addr)? as i32 as i64;
                frame.stack.push_i64(value);
            }

            // Extending load, unsigned
            Op::Load8Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u8(addr)? as i32;
                frame.stack.push_i32(value);
            }
            Op::Load16Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u16(addr)? as i32;
                frame.stack.push_i32(value);
            }
            Op::Load8I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u8(addr)? as i64;
                frame.stack.push_i64(value);
            }
            Op::Load16I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u16(addr)? as i64;
                frame.stack.push_i64(value);
            }
            Op::Load32I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                let value = memory.get_u32(addr)? as i64;
                frame.stack.push_i64(value);
            }
            Op::StoreI32(addr) => {
                let value = frame.stack.pop_i32()?;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_i32(addr, value)?;
            }
            Op::StoreI64(addr) => {
                let value = frame.stack.pop_i64()?;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_i64(addr, value)?;
            }
            Op::StoreF32(addr) => {
                let value = frame.stack.pop_f32()?;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_f32(addr, value)?;
            }
            Op::StoreF64(addr) => {
                let value = frame.stack.pop_f64()?;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_f64(addr, value)?;
            }

            // Silently narrow the width of the value
            Op::Store8_32(addr) => {
                let value = frame.stack.pop_i32()? as u8;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_u8(addr, value)?;
            }
            Op::Store16_32(addr) => {
                let value = frame.stack.pop_i32()? as u16;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_u16(addr, value)?;
            }
            Op::Store8_64(addr) => {
                let value = frame.stack.pop_i64()? as u8;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_u8(addr, value)?;
            }
            Op::Store16_64(addr) => {
                let value = frame.stack.pop_i64()? as u16;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_u16(addr, value)?;
            }
            Op::Store32_64(addr) => {
                let value = frame.stack.pop_i64()? as u32;
                let addr = adjust_memarg(&mut frame.stack, &addr)?;
                memory.set_u32(addr, value)?;
            }

            Op::I32Const(v) => {
                frame.stack.push_i32(v);
            }
            Op::I64Const(v) => {
                frame.stack.push_i64(v);
            }
            Op::F32Const(v) => {
                frame.stack.push_f32(v);
            }
            Op::F64Const(v) => {
                frame.stack.push_f64(v);
            }
            Op::MemorySize => {
                let size = memory.size();
                frame.stack.push_u32(size as u32);
            }
            Op::MemoryGrow => {
                let delta = frame.stack.pop_i32()?;
                if delta < 0 {
                    return Err(Fault::CannotGrowMemory);
                }
                let old_size = memory.grow(delta as usize)?;
                frame.stack.push_i32(old_size as i32);
            }
            Op::I32Eqz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(if value == 0 { 1 } else { 0 });
            }
            Op::I32Eq => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a == b { 1 } else { 0 });
            }
            Op::I32Ne => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a != b { 1 } else { 0 });
            }
            Op::I32LtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::I32LtU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::I32GtS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::I32GtU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::I32LeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::I32LeU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::I32GeS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::I32GeU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::I64Eqz => {
                let value = frame.stack.pop_i64()?;
                frame.stack.push_i32(if value == 0 { 1 } else { 0 });
            }
            Op::I64Eq => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a == b { 1 } else { 0 });
            }
            Op::I64Ne => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a != b { 1 } else { 0 });
            }
            Op::I64LtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::I64LtU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::I64GtS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::I64GtU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::I64LeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::I64LeU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::I64GeS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::I64GeU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::F32Eq => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a == b { 1 } else { 0 });
            }
            Op::F32Ne => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a != b { 1 } else { 0 });
            }
            Op::F32Lt => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::F32Gt => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::F32Le => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::F32Ge => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::F64Eq => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a == b { 1 } else { 0 });
            }
            Op::F64Ne => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a != b { 1 } else { 0 });
            }
            Op::F64Lt => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a < b { 1 } else { 0 });
            }
            Op::F64Gt => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a > b { 1 } else { 0 });
            }
            Op::F64Le => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a <= b { 1 } else { 0 });
            }
            Op::F64Ge => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_i32(if a >= b { 1 } else { 0 });
            }
            Op::I32Clz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(value.leading_zeros() as i32);
            }
            Op::I32Ctz => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(value.trailing_zeros() as i32);
            }
            Op::I32Popcnt => {
                let value = frame.stack.pop_i32()?;
                frame.stack.push_i32(value.count_ones() as i32);
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
                frame.stack.push_i32(a.wrapping_div(b));
            }
            Op::I32DivU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(a.wrapping_div(b) as i32);
            }
            Op::I32RemS => {
                let b = frame.stack.pop_i32()?;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_rem(b));
            }
            Op::I32RemU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(a.wrapping_rem(b) as i32);
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
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shl(b));
            }
            Op::I32ShrS => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.wrapping_shr(b));
            }
            Op::I32ShrU => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()? as u32;
                frame.stack.push_i32(a.wrapping_shr(b) as i32);
            }
            Op::I32Rotl => {
                let b = frame.stack.pop_i32()? as u32;
                let a = frame.stack.pop_i32()?;
                frame.stack.push_i32(a.rotate_left(b));
            }
            Op::I32Rotr => {
                let b = frame.stack.pop_i32()? as u32;
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
                frame.stack.push_i64(a.wrapping_div(b));
            }
            Op::I64DivU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i64(a.wrapping_div(b) as i64);
            }
            Op::I64RemS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_rem(b));
            }
            Op::I64RemU => {
                let b = frame.stack.pop_i64()? as u64;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i64(a.wrapping_rem(b) as i64);
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
                let b = frame.stack.pop_i64()? as u32;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shl(b));
            }
            Op::I64ShrS => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.wrapping_shr(b as u32));
            }
            Op::I64ShrU => {
                let b = frame.stack.pop_i64()? as u32;
                let a = frame.stack.pop_i64()? as u64;
                frame.stack.push_i64(a.wrapping_shr(b) as i64);
            }
            Op::I64Rotl => {
                let b = frame.stack.pop_i64()?;
                let a = frame.stack.pop_i64()?;
                frame.stack.push_i64(a.rotate_left(b as u32));
            }
            Op::I64Rotr => {
                let b = frame.stack.pop_i64()?;
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

                frame.stack.push_f32(min_nan_correct(a, b));
            }
            Op::F32Max => {
                let b = frame.stack.pop_f32()?;
                let a = frame.stack.pop_f32()?;
                frame.stack.push_f32(max_nan_correct(a, b));
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
                frame.stack.push_f64(min_nan_correct(a, b));
            }
            Op::F64Max => {
                let b = frame.stack.pop_f64()?;
                let a = frame.stack.pop_f64()?;
                frame.stack.push_f64(max_nan_correct(a, b));
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
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF32U => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_u32(value as u32);
            }
            Op::I32TruncF64S => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF64U => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_u32(value as u32);
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
                frame.stack.push_i64(value as i64);
            }
            Op::I64TruncF32U => {
                let value = frame.stack.pop_f32()?;
                frame.stack.push_u64(value as u64);
            }
            Op::I64TruncF64S => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_i64(value as i64);
            }
            Op::I64TruncF64U => {
                let value = frame.stack.pop_f64()?;
                frame.stack.push_u64(value as u64);
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
        }
    }
}

fn adjust_memarg(stack: &mut Stack, memarg: &MemArg) -> Result<usize, Fault> {
    let base_addr = stack.pop_i32()? as usize;

    // Note: Alignment is only a "hint", we could issue a warning here, but that would just slow
    //  down the interpreter.

    Ok(memarg.offset + base_addr)
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
    Unit,
}

impl Eq for Value {}

impl Value {
    pub(crate) fn type_of(&self) -> ValueType {
        match self {
            Value::I32(_) => ValueType::I32,
            Value::I64(_) => ValueType::I64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::V128(_) => ValueType::V128,
            Unit => ValueType::Unit,
        }
    }

    pub fn pop_from(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => {
                stack.pop_u64()?;
                Unit
            }
            ValueType::I32 => Value::I32(stack.pop_i32()?),
            ValueType::I64 => Value::I64(stack.pop_i64()?),
            ValueType::F32 => Value::F32(stack.pop_f32()?),
            ValueType::F64 => Value::F64(stack.pop_f64()?),
            ValueType::V128 => {
                let (l, r) = (stack.pop_u64()?, stack.pop_u64()?);
                Value::V128((r as u128) << 64 | l as u128)
            }
            ValueType::FuncRef => unimplemented!("Function references not supported"),
            ValueType::ExternRef => unimplemented!("Extern references not supported"),
        })
    }

    pub fn top_of(ty: ValueType, stack: &mut Stack) -> Result<Self, Fault> {
        Ok(match ty {
            ValueType::Unit => {
                stack.pop_u64()?;
                Unit
            }
            ValueType::I32 => Value::I32(stack.top_i32()?),
            ValueType::I64 => Value::I64(stack.top_i64()?),
            ValueType::F32 => Value::F32(stack.top_f32()?),
            ValueType::F64 => Value::F64(stack.top_f64()?),
            ValueType::V128 => {
                let (l, r) = (stack.top_u64()?, stack.top_u64()?);
                Value::V128((r as u128) << 64 | l as u128)
            }
            ValueType::FuncRef => unimplemented!("Function references not supported"),
            ValueType::ExternRef => unimplemented!("Extern references not supported"),
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
            Unit => {
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

// For executing little fragments of code e.g. globals or data segments
pub(crate) fn exec_fragment(program: &[u8], return_type: ValueType) -> Result<Value, Fault> {
    let const_program = decode(program).map_err(|_| Fault::DecodeError)?;
    let return_types = vec![return_type];
    let mut global_exec_frame = Frame {
        locals: vec![Unit; 0],
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
    let mut const_prg_globals = vec![];

    // In this case the expectation is we run out of instructions, and the stack contains the return
    // value.
    let mut ticks_used = 0;
    let result = execute(
        &mut global_exec_frame,
        &mut const_prg_memory,
        &mut const_prg_globals,
        EXPR_TICK_LIMIT,
        &[],
        &mut ticks_used,
    )?;
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

impl Display for ExecError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::LinkageError(e) => write!(f, "Linkage error: {}", e),
            ExecError::ExecutionFault(e) => write!(f, "Execution fault: {}", e),
        }
    }
}

impl Error for ExecError {}

pub struct ExecutionLimits {
    pub tick_limit: usize,
    pub stack_depth_limit: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            tick_limit: 100_000_000,
            stack_depth_limit: 100000,
        }
    }
}
/// A context for executing functions in an Instance derived from a module.
pub struct Execution<M>
where
    M: Memory,
{
    /// The linked module.
    // TODO: in the future this could be multiple instances?, one per module.
    instance: Instance,
    /// The stack of frames for the current execution.
    frame_stack: Vec<Frame>,
    /// The memory for the current execution.
    memory: M,
    /// Final result of execution when all frames have executed.
    result: Option<Vec<Value>>,
    /// Execution limits constants (tick limit, stack depth, etc.)
    limits: ExecutionLimits,
    /// Current cumulative executed tick count
    consumed_ticks: usize,
}

impl<M> Execution<M>
where
    M: Memory,
{
    pub fn new(linkage: Instance, memory: M, limits: ExecutionLimits) -> Self {
        Execution {
            instance: linkage,
            frame_stack: vec![],
            memory,
            result: None,
            limits,
            consumed_ticks: 0,
        }
    }

    pub fn reset_ticks(&mut self) {
        self.consumed_ticks = 0;
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn result(&self) -> Option<&[Value]> {
        self.result.as_deref()
    }

    pub fn prepare(&mut self, funcidx: u32, args: &[Value]) -> Result<(), ExecError> {
        self.frame_stack.clear();
        let frame = self
            .instance
            .frame_for_funcidx(funcidx, args)
            .map_err(ExecError::LinkageError)?;

        self.frame_stack.push(frame);
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), ExecError> {
        loop {
            let stack_depth = self.frame_stack.len();
            let top_frame = self.frame_stack.last_mut().unwrap();
            let result = execute(
                top_frame,
                &mut self.memory,
                &mut self.instance.globals,
                self.limits.tick_limit,
                &self.instance.module.types,
                &mut self.consumed_ticks,
            );
            match result {
                Ok(Continuation::ProgramEnd) | Ok(Continuation::DoneReturn) => {
                    let mut return_values = vec![];
                    // get the return results based on the return types
                    // Theses are in reverse order on the stack...
                    for rt in top_frame.return_types.iter().rev() {
                        return_values.push((
                            *rt,
                            Value::pop_from(*rt, &mut top_frame.stack)
                                .map_err(ExecError::ExecutionFault)?,
                        ));
                    }
                    self.frame_stack.pop();
                    if let Some(frame) = self.frame_stack.last_mut() {
                        for (_, v) in return_values.iter().rev() {
                            v.push_to(&mut frame.stack);
                        }
                        continue;
                    } else {
                        self.result =
                            Some(return_values.into_iter().map(|(_, v)| v).rev().collect());
                        return Ok(());
                    }
                }
                Ok(Continuation::Call(funcidx, typesig)) => {
                    // Can't go deeper than stack limits.
                    if stack_depth >= self.limits.stack_depth_limit {
                        return Err(ExecError::ExecutionFault(Fault::CallStackDepthLimit(
                            self.limits.stack_depth_limit,
                            stack_depth,
                        )));
                    }

                    // Pop args from stack, depending on the function signature
                    let function_num = self.instance.module.functions[funcidx as usize];
                    let typesig = typesig.map(|t| t as usize);
                    let functype = &self.instance.module.types[typesig.unwrap_or(function_num)];

                    let num_args = functype.params.len();
                    let mut args = vec![Unit; num_args];
                    for (i, param) in functype.params.iter().rev().enumerate() {
                        let value = Value::pop_from(*param, &mut top_frame.stack)
                            .map_err(ExecError::ExecutionFault)?;
                        // Verify the value type matches for the position in the signature
                        if value.type_of() != *param {
                            return Err(ExecError::ExecutionFault(Fault::ArgumentTypeMismatch(
                                value.type_of(),
                                *param,
                            )));
                        }
                        args[num_args - i - 1] = value;
                    }
                    let funcidx = funcidx as usize;
                    if funcidx >= self.instance.programs.len() {
                        return Err(ExecError::LinkageError(LinkError::FunctionNotFound));
                    }
                    let program = &self.instance.programs[funcidx];
                    let num_locals = program.local_types.len();
                    let mut locals = args.to_vec();
                    if num_locals > args.len() {
                        locals.extend_from_slice(&vec![Unit; num_locals - args.len()]);
                    }
                    let return_types = program.return_types.clone();
                    let frame = Frame {
                        locals,
                        return_types,
                        program: program.clone(),
                        stack: Stack::new(),
                        pc: 0,
                        control_stack: vec![],
                    };
                    self.frame_stack.push(frame);
                }

                Err(fault) => return Err(ExecError::ExecutionFault(fault)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::exec::{Execution, ExecutionLimits, Value};
    use crate::instance::mk_instance;
    use crate::module::Module;

    #[test]
    fn load_run_itoa() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();

        let linked = mk_instance(module).unwrap();
        let memory = linked.memories[0].clone();
        let mut execution = Execution::new(linked, memory, ExecutionLimits::default());
        execution.prepare(1, &[Value::I32(123)]).unwrap();
        execution.run().unwrap();
    }
}
