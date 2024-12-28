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

use crate::decode::decode;
use crate::frame::Frame;
use crate::link::WASM_PAGE_SIZE;
use crate::memory::Memory;
use crate::module::Global;
use crate::op::{MemArg, Op};
use crate::stack::Stack;
use crate::ValueType;

pub fn execute<'a>(
    frame: &mut Frame,
    memory: &'a mut Memory<'a>,
    globals: &mut [GlobalVar],
    num_ticks: usize,
) {
    let mut ticks_used = 0;
    loop {
        // Pull next opcode from the program
        let pc = frame.pc;
        if pc >= frame.program.ops.len() {
            // We've reached the end of the program
            break;
        }
        ticks_used += 1;
        if ticks_used >= num_ticks {
            panic!("Exceeded tick limit");
        }
        frame.pc += 1;
        let op = frame.program.ops[pc].clone();

        match op {
            Op::Nop => {}
            Op::StartScope(sig, scope_type, label) => {
                frame.push_control(sig, scope_type, label);
            }
            Op::EndScope(_st) => {
                let end_scope = frame.pop_control();
                // assert_eq!(st, end_scope.scope_type);
                // Shrink-stack to the width declared in the control scope.
                frame.stack.shrink_to(end_scope.stack_width);
                // TODO: do we need to do something with the signature/value ?
            }
            Op::If(else_label) => {
                // Pop condition from stack, evaluate.
                // Then attempt to jump to else_label if false. If that fails, jump to end_label.
                let condition = frame.stack.pop_u32();
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
                let pop_depth = frame
                    .control_stack
                    .iter()
                    .rev()
                    .position(|c| c.label == label)
                    .unwrap();
                if pop_depth != 0 {
                    for _ in 0..pop_depth - 1 {
                        frame.pop_control();
                    }
                }
                assert!(frame.jump_label(label));
            }
            Op::BrIf(label) => {
                let condition = frame.stack.pop_u32();
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
                            frame.pop_control();
                        }
                    }

                    assert!(frame.jump_label(label));
                }
            }
            Op::BrTable(table, default) => {
                // TODO: We gotta pop the control stack back to the label we're jumping to.
                //   But to do that we'd need to know the label for each scope.

                let index = frame.stack.pop_u32() as usize;
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
                    frame.pop_control();
                }
                return;
            }
            Op::Call(_) => {
                todo!();
            }
            Op::CallIndirect(_) => {
                todo!();
            }
            Op::Drop => {
                frame.stack.pop_u32();
            }
            Op::Select => {
                //The select instruction returns its first operand if $condition is true, or its second operand otherwise.
                let condition = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                let b = frame.stack.pop_u32();
                if condition != 0 {
                    frame.stack.push_u32(a);
                } else {
                    frame.stack.push_u32(b);
                }
            }
            Op::GetLocal(idx) => {
                frame.push_local_to_stack(idx);
            }
            Op::SetLocal(idx) => {
                frame.set_local_from_stack(idx, true);
            }
            Op::TeeLocal(idx) => {
                frame.set_local_from_stack(idx, false);
            }
            Op::GetGlobal(g) => {
                let gv = &globals[g as usize];
                gv.value.push_to(&mut frame.stack);
            }
            Op::LoadI32(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_i32(addr);
                frame.stack.push_i32(value);
            }
            Op::LoadI64(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_i64(addr);
                frame.stack.push_i64(value);
            }
            Op::LoadF32(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_f32(addr);
                frame.stack.push_u32(value.to_bits());
            }
            Op::LoadF64(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_f64(addr);
                frame.stack.push_u64(value.to_bits());
            }

            // Extending load, signed
            Op::Load8SE(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u8(addr) as i8 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load16Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u16(addr) as i16 as i32;
                frame.stack.push_i32(value);
            }
            Op::Load8I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u8(addr) as i8 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load16I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u16(addr) as i16 as i64;
                frame.stack.push_i64(value);
            }
            Op::Load32I64Se(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u32(addr) as i32 as i64;
                frame.stack.push_i64(value);
            }

            // Extending load, unsigned
            Op::Load8Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u8(addr) as u32;
                frame.stack.push_u32(value);
            }
            Op::Load16Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u16(addr) as u32;
                frame.stack.push_u32(value);
            }
            Op::Load8I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u8(addr) as u64;
                frame.stack.push_u64(value);
            }
            Op::Load16I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u16(addr) as u64;
                frame.stack.push_u64(value);
            }
            Op::Load32I64Ze(addr) => {
                let addr = adjust_memarg(&mut frame.stack, &addr);
                let value = memory.get_u32(addr) as u64;
                frame.stack.push_u64(value);
            }
            Op::StoreI32(addr) => {
                let value = frame.stack.pop_i32();
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_i32(addr, value);
            }
            Op::StoreI64(addr) => {
                let value = frame.stack.pop_i64();
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_i64(addr, value);
            }
            Op::StoreF32(addr) => {
                let value = frame.stack.pop_f32();
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_f32(addr, value);
            }
            Op::StoreF64(addr) => {
                let value = frame.stack.pop_f64();
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_f64(addr, value);
            }

            // Silently narrow the width of the value
            Op::Store8_32(addr) => {
                let value = frame.stack.pop_i32() as u8;
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_u8(addr, value);
            }
            Op::Store16_32(addr) => {
                let value = frame.stack.pop_i32() as u16;
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_u16(addr, value);
            }
            Op::Store8_64(addr) => {
                let value = frame.stack.pop_i64() as u8;
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_u8(addr, value);
            }
            Op::Store16_64(addr) => {
                let value = frame.stack.pop_i64() as u16;
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_u16(addr, value);
            }
            Op::Store32_64(addr) => {
                let value = frame.stack.pop_i64() as u32;
                let addr = adjust_memarg(&mut frame.stack, &addr);
                memory.set_u32(addr, value);
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
            Op::MemorySize => {
                let size = memory.size();
                frame.stack.push_u32(size as u32);
            }
            Op::MemoryGrow => {
                panic!("MemoryGrow not implemented");
            }
            Op::I32Eqz => {
                let value = frame.stack.pop_i32();
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
            }
            Op::I32Eq => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::I32Ne => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::I32LtS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I32LtU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I32GtS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I32GtU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I32LeS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I32LeU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I32GeS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I32GeU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I64Eqz => {
                let value = frame.stack.pop_i64();
                frame.stack.push_u32(if value == 0 { 1 } else { 0 });
            }
            Op::I64Eq => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::I64Ne => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::I64LtS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I64LtU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::I64GtS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I64GtU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::I64LeS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I64LeU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::I64GeS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I64GeU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::F32Eq => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::F32Ne => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::F32Lt => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::F32Gt => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::F32Le => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::F32Ge => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::F64Eq => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a == b { 1 } else { 0 });
            }
            Op::F64Ne => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a != b { 1 } else { 0 });
            }
            Op::F64Lt => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a < b { 1 } else { 0 });
            }
            Op::F64Gt => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a > b { 1 } else { 0 });
            }
            Op::F64Le => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a <= b { 1 } else { 0 });
            }
            Op::F64Ge => {
                let b = frame.stack.pop_f64();
                let a = frame.stack.pop_f64();
                frame.stack.push_u32(if a >= b { 1 } else { 0 });
            }
            Op::I32Clz => {
                let value = frame.stack.pop_i32();
                frame.stack.push_u32(value.leading_zeros());
            }
            Op::I32Ctz => {
                let value = frame.stack.pop_i32();
                frame.stack.push_u32(value.trailing_zeros());
            }
            Op::I32Popcnt => {
                let value = frame.stack.pop_i32();
                frame.stack.push_u32(value.count_ones());
            }
            Op::I32Add => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_add(b));
            }
            Op::I32Sub => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_sub(b));
            }
            Op::I32Mul => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_mul(b));
            }
            Op::I32DivS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_div(b));
            }
            Op::I32DivU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(a.wrapping_div(b));
            }
            Op::I32RemS => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_rem(b));
            }
            Op::I32RemU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(a.wrapping_rem(b));
            }
            Op::I32And => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a & b);
            }
            Op::I32Or => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a | b);
            }
            Op::I32Xor => {
                let b = frame.stack.pop_i32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a ^ b);
            }
            Op::I32Shl => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_shl(b));
            }
            Op::I32ShrS => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.wrapping_shr(b));
            }
            Op::I32ShrU => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_u32();
                frame.stack.push_u32(a.wrapping_shr(b));
            }
            Op::I32Rotl => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.rotate_left(b));
            }
            Op::I32Rotr => {
                let b = frame.stack.pop_u32();
                let a = frame.stack.pop_i32();
                frame.stack.push_i32(a.rotate_right(b));
            }
            Op::I64Clz => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value.leading_zeros() as i64);
            }
            Op::I64Ctz => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value.trailing_zeros() as i64);
            }
            Op::I64Popcnt => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value.count_ones() as i64);
            }
            Op::I64Add => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_add(b));
            }
            Op::I64Sub => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_sub(b));
            }
            Op::I64Mul => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_mul(b));
            }
            Op::I64DivS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_div(b));
            }
            Op::I64DivU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u64(a.wrapping_div(b));
            }
            Op::I64RemS => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_rem(b));
            }
            Op::I64RemU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u64(a.wrapping_rem(b));
            }
            Op::I64And => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a & b);
            }
            Op::I64Or => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a | b);
            }
            Op::I64Xor => {
                let b = frame.stack.pop_i64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a ^ b);
            }
            Op::I64Shl => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_shl(b as u32));
            }
            Op::I64ShrS => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.wrapping_shr(b as u32));
            }
            Op::I64ShrU => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_u64();
                frame.stack.push_u64(a.wrapping_shr(b as u32));
            }
            Op::I64Rotl => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.rotate_left(b as u32));
            }
            Op::I64Rotr => {
                let b = frame.stack.pop_u64();
                let a = frame.stack.pop_i64();
                frame.stack.push_i64(a.rotate_right(b as u32));
            }
            Op::F32Abs => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.abs());
            }
            Op::F32Neg => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(-value);
            }
            Op::F32Ceil => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.ceil());
            }
            Op::F32Floor => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.floor());
            }
            Op::F32Trunc => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.trunc());
            }
            Op::F32Nearest => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.round());
            }
            Op::F32Sqrt => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f32(value.sqrt());
            }
            Op::F32Add => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a + b);
            }
            Op::F32Sub => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a - b);
            }
            Op::F32Mul => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a * b);
            }
            Op::F32Div => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a / b);
            }
            Op::F32Min => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a.min(b));
            }
            Op::F32Max => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a.max(b));
            }
            Op::F32Copysign => {
                let b = frame.stack.pop_f32();
                let a = frame.stack.pop_f32();
                frame.stack.push_f32(a.copysign(b));
            }
            Op::I32WrapI64 => {
                let value = frame.stack.pop_i64();
                // Turn to i32, wrapping around if necessary
                // TODO: I think this is wrong
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF32S => {
                let value = frame.stack.pop_f32();
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF32U => {
                let value = frame.stack.pop_f32();
                frame.stack.push_u32(value as u32);
            }
            Op::I32TruncF64S => {
                let value = frame.stack.pop_f64();
                frame.stack.push_i32(value as i32);
            }
            Op::I32TruncF64U => {
                let value = frame.stack.pop_f64();
                frame.stack.push_u32(value as u32);
            }
            Op::I64ExtendI32S => {
                let value = frame.stack.pop_i32();
                frame.stack.push_i64(value as i64);
            }
            Op::I64ExtendI32U => {
                let value = frame.stack.pop_u32();
                frame.stack.push_u64(value as u64);
            }
            Op::I64TruncF32S => {
                let value = frame.stack.pop_f32();
                frame.stack.push_i64(value as i64);
            }
            Op::I64TruncF32U => {
                let value = frame.stack.pop_f32();
                frame.stack.push_u64(value as u64);
            }
            Op::I64TruncF64S => {
                let value = frame.stack.pop_f64();
                frame.stack.push_i64(value as i64);
            }
            Op::I64TruncF64U => {
                let value = frame.stack.pop_f64();
                frame.stack.push_u64(value as u64);
            }
            Op::F32ConvertI32S => {
                let value = frame.stack.pop_i32();
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI32U => {
                let value = frame.stack.pop_u32();
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI64S => {
                let value = frame.stack.pop_i64();
                frame.stack.push_f32(value as f32);
            }
            Op::F32ConvertI64U => {
                let value = frame.stack.pop_u64();
                frame.stack.push_f32(value as f32);
            }
            Op::F32DemoteF64 => {
                let value = frame.stack.pop_f64();
                frame.stack.push_f32(value as f32);
            }
            Op::F64ConvertI32S => {
                let value = frame.stack.pop_i32();
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI32U => {
                let value = frame.stack.pop_u32();
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI64S => {
                let value = frame.stack.pop_i64();
                frame.stack.push_f64(value as f64);
            }
            Op::F64ConvertI64U => {
                let value = frame.stack.pop_u64();
                frame.stack.push_f64(value as f64);
            }
            Op::F64PromoteF32 => {
                let value = frame.stack.pop_f32();
                frame.stack.push_f64(value as f64);
            }
            Op::I32ReinterpretF32 => {
                let value = frame.stack.pop_f32();
                frame.stack.push_u32(value.to_bits());
            }
            Op::I64ReinterpretF64 => {
                let value = frame.stack.pop_f64();
                frame.stack.push_u64(value.to_bits());
            }
            Op::F32ReinterpretI32 => {
                let value = frame.stack.pop_u32();
                frame.stack.push_f32(f32::from_bits(value));
            }
            Op::F64ReinterpretI64 => {
                let value = frame.stack.pop_u64();
                frame.stack.push_f64(f64::from_bits(value));
            }
            Op::I32Extend8S => {
                let value = frame.stack.pop_i32();
                frame.stack.push_i32(value as i8 as i32);
            }
            Op::I32Extend16S => {
                let value = frame.stack.pop_i32();
                frame.stack.push_i32(value as i16 as i32);
            }
            Op::I64Extend8S => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value as i8 as i64);
            }
            Op::I64Extend16S => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value as i16 as i64);
            }
            Op::I64Extend32S => {
                let value = frame.stack.pop_i64();
                frame.stack.push_i64(value as i32 as i64);
            }
        }
    }
}

fn adjust_memarg(stack: &mut Stack, memarg: &MemArg) -> usize {
    let i = stack.pop_i32() as usize;

    // TODO: handle align
    memarg.offset + i
}

#[derive(Debug, Clone)]
pub struct GlobalVar {
    pub decl: Global,
    pub value: Value,
}

#[derive(Debug, Clone, Copy)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    V128(u128),
    Unit,
}

impl Value {
    pub(crate) fn type_of(&self) -> ValueType {
        match self {
            Value::I32(_) => ValueType::I32,
            Value::I64(_) => ValueType::I64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::V128(_) => ValueType::V128,
            Value::Unit => ValueType::Unit,
        }
    }

    pub fn pop_to(ty: ValueType, stack: &mut Stack) -> Self {
        match ty {
            ValueType::Unit => {
                stack.pop_u64();
                Value::Unit
            }
            ValueType::I32 => Value::I32(stack.pop_i32()),
            ValueType::I64 => Value::I64(stack.pop_i64()),
            ValueType::F32 => Value::F32(stack.pop_f32()),
            ValueType::F64 => Value::F64(stack.pop_f64()),
            ValueType::V128 => {
                let (l, r) = (stack.pop_u64(), stack.pop_u64());
                Value::V128((r as u128) << 64 | l as u128)
            }
            ValueType::FuncRef => unimplemented!("Function references not supported"),
            ValueType::ExternRef => unimplemented!("Extern references not supported"),
        }
    }

    pub fn top_to(ty: ValueType, stack: &mut Stack) -> Self {
        match ty {
            ValueType::Unit => {
                stack.pop_u64();
                Value::Unit
            }
            ValueType::I32 => Value::I32(stack.top_i32()),
            ValueType::I64 => Value::I64(stack.top_i64()),
            ValueType::F32 => Value::F32(stack.top_f32()),
            ValueType::F64 => Value::F64(stack.top_f64()),
            ValueType::V128 => {
                let (l, r) = (stack.top_u64(), stack.top_u64());
                Value::V128((r as u128) << 64 | l as u128)
            }
            ValueType::FuncRef => unimplemented!("Function references not supported"),
            ValueType::ExternRef => unimplemented!("Extern references not supported"),
        }
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
            Value::Unit => {
                stack.push_u64(0);
            }
        }
    }
}

// For executing little fragments of code e.g. globals or data segments
pub(crate) fn exec_fragment(program: &[u8], return_type: ValueType) -> Value {
    let const_program = decode(program).unwrap();
    let mut global_exec_frame = Frame {
        locals: vec![Value::Unit; 0],
        program: const_program,
        stack: Stack::new(),
        pc: 0,
        control_stack: vec![],
    };
    // This little fragment, it doesn't get much memory and doesn't get *any* globals.
    // TODO: I don't actually know what a reasonable amount of memory is, so we'll just default
    //   to one page.
    let mut const_prg_memory_vec = vec![0; WASM_PAGE_SIZE];
    let mut const_prg_memory = Memory::new(&mut const_prg_memory_vec);
    let mut const_prg_globals = vec![];
    execute(
        &mut global_exec_frame,
        &mut const_prg_memory,
        &mut const_prg_globals,
        1000,
    );
    Value::pop_to(return_type, &mut global_exec_frame.stack)
}

#[cfg(test)]
mod tests {
    use crate::exec::{execute, Value};
    use crate::link::link;
    use crate::memory::Memory;
    use crate::module::Module;

    #[test]
    fn load_run_itoa() {
        let module_data: Vec<u8> = include_bytes!("../tests/itoa.wasm").to_vec();
        let module = Module::load(&module_data).unwrap();

        let linked = link(module);
        let mut memory_vec = linked.memories[0].clone();
        let mut globals = linked.globals.clone();
        let mut frame = linked
            .frame_for_funcname("itoa", &[Value::I32(123)])
            .unwrap();

        let mut memory = Memory::new(&mut memory_vec);
        execute(&mut frame, &mut memory, &mut globals, 10000);

        // Stack should be empty after execution.
        assert_eq!(frame.stack.width(), 0);
        // Check that the memory contains the expected string.
        let expected = [49, 50, 51, 0];
        assert_eq!(expected, memory_vec[8010..8014]);
    }
}
