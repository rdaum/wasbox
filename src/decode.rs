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

use crate::module::LEB128Reader;
use crate::op::{MemArg, Op};
use crate::opcode::OpCode;
use crate::ValueType;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub ops: Vec<Op>,
    pub labels: Labels,
    pub local_types: Vec<ValueType>,
    pub return_types: Vec<ValueType>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    InvalidOpcode(u8),
    UnimplementedOpcode(u8, String),
    InvalidSignature(u32),
    FailedToDecode(String),
    InvalidDataSegmentType(u32),
    UnsupportedType(u32, String),
    MalformedMemory(String),
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::InvalidOpcode(opcode) => {
                write!(f, "Invalid opcode: {:#0x}", opcode)
            }
            DecodeError::UnimplementedOpcode(opcode, reason) => {
                write!(f, "Unimplemented opcode: {:#0x} - {}", opcode, reason)
            }
            DecodeError::InvalidSignature(signature) => {
                write!(f, "Invalid signature: {:#0x}", signature)
            }
            DecodeError::FailedToDecode(reason) => {
                write!(f, "Failed to decode: {}", reason)
            }
            DecodeError::InvalidDataSegmentType(ty) => {
                write!(f, "Invalid data segment type: {:#0x}", ty)
            }
            DecodeError::UnsupportedType(ty, reason) => {
                write!(f, "Unsupported type: {:#0x} - {}", ty, reason)
            }
            DecodeError::MalformedMemory(reason) => {
                write!(f, "Malformed memory: {}", reason)
            }
        }
    }
}

impl Error for DecodeError {}

fn read_memarg(reader: &mut LEB128Reader) -> Result<MemArg, DecodeError> {
    // align, offset in mem
    let align = reader.load_imm_varuint32()?;
    let offset = reader.load_imm_varuint32()? as usize;
    Ok(MemArg { align, offset })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    labels: Vec<Label>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    position: Option<usize>,
}

impl Label {
    fn new() -> Self {
        Label { position: None }
    }

    fn bound(position: usize) -> Self {
        Label {
            position: Some(position),
        }
    }

    fn bind(&mut self, position: usize) {
        self.position = Some(position);
    }

    #[allow(dead_code)]
    fn unbind(&mut self) {
        self.position = None;
    }

    /// Return the position for a bound label
    pub fn position(&self) -> Option<usize> {
        self.position
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScopeType {
    Program,
    Loop,
    Block,
    IfElse,
}

pub type LabelId = usize;

struct Scope {
    scope_type: ScopeType,
    #[allow(dead_code)]
    signature: ValueType,
    label: LabelId,
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

impl Program {
    pub fn new() -> Self {
        Program {
            ops: vec![],
            labels: Labels { labels: vec![] },
            local_types: vec![],
            return_types: vec![],
        }
    }

    pub fn push(&mut self, op: Op) {
        self.ops.push(op);
    }
}

impl Labels {
    pub fn new_bound_label(&mut self, pos: usize) -> LabelId {
        let label = Label::bound(pos);
        self.labels.push(label);
        (self.labels.len() - 1) as _
    }

    pub fn new_unbound_label(&mut self) -> LabelId {
        let label = Label::new();
        self.labels.push(label);
        (self.labels.len() - 1) as _
    }

    pub fn bind_label(&mut self, label: LabelId, position: usize) {
        assert!(self.labels[label].position.is_none());
        self.labels[label].bind(position);
    }

    pub fn find_label(&self, label: LabelId) -> Option<usize> {
        self.labels[label].position()
    }

    fn mk_program(&mut self) -> Scope {
        let label = self.new_bound_label(0);
        Scope {
            scope_type: ScopeType::Program,
            signature: ValueType::Unit,
            label,
        }
    }

    fn mk_loop(&mut self, signature: ValueType) -> Scope {
        let label = self.new_unbound_label();
        Scope {
            scope_type: ScopeType::Loop,
            signature,
            label,
        }
    }

    fn mk_block(&mut self, signature: ValueType) -> Scope {
        let label = self.new_unbound_label();
        Scope {
            scope_type: ScopeType::Block,
            signature,
            label,
        }
    }

    fn mk_if_else(&mut self, signature: ValueType) -> Scope {
        let label = self.new_unbound_label();
        Scope {
            scope_type: ScopeType::IfElse,
            signature,
            label,
        }
    }
}

pub fn decode(program_stream: &[u8]) -> Result<Program, DecodeError> {
    let mut prg = Program::new();
    // The assumption is that program_stream is after locals, where the opcodes begin.
    let mut reader = LEB128Reader::new(program_stream, 0);

    let mut scope_stack = vec![prg.labels.mk_program()];

    // Decode the raw program stream and translate it into our ADT Op
    while reader.remaining() != 0 {
        let opcode_o = reader.load_imm_u8()?;
        let opcode: OpCode =
            OpCode::from_repr(opcode_o).ok_or(DecodeError::InvalidOpcode(opcode_o))?;

        // Block-stack... we need to keep track of the current block we're while decoding.
        match opcode {
            OpCode::Unreachable => {
                continue;
            }
            OpCode::Nop => {
                prg.push(Op::Nop);
            }

            OpCode::Block => {
                let signature = ValueType::read(&mut reader)?;
                let block = prg.labels.mk_block(signature);
                let label = block.label;
                scope_stack.push(block);
                prg.push(Op::StartScope(signature, ScopeType::Block, label));
            }
            OpCode::Loop => {
                let signature = ValueType::read(&mut reader)?;
                let block = prg.labels.mk_loop(signature);
                prg.push(Op::StartScope(signature, ScopeType::Loop, block.label));
                prg.labels.bind_label(block.label, prg.ops.len());
                scope_stack.push(block);
            }
            OpCode::If => {
                let signature = ValueType::read(&mut reader)?;
                let block = prg.labels.mk_if_else(signature);

                prg.push(Op::StartScope(signature, ScopeType::IfElse, block.label));

                // Emit If opcode with the start label bound, and unbound else and end label.
                prg.push(Op::If(block.label));

                scope_stack.push(block);
            }
            OpCode::Else => {
                // The last block on the stack should be an IfBlock, otherwise that's corrupt program.
                let if_block = scope_stack.last_mut().unwrap();
                assert_eq!(if_block.scope_type, ScopeType::IfElse);

                // Push the Else opcode to jump to end of the if block if we hit this.
                prg.push(Op::Else(if_block.label));

                // Bind if block label to the current position.
                prg.labels.bind_label(if_block.label, prg.ops.len());
            }
            OpCode::End => {
                let block = scope_stack.pop().unwrap();

                // Bind to the end-label if not already bound.
                if prg.labels.find_label(block.label).is_none() {
                    prg.labels.bind_label(block.label, prg.ops.len());
                }

                // Always push an EndScope.
                prg.push(Op::EndScope(block.scope_type));
            }

            OpCode::Br => {
                let depth = reader.load_imm_varuint32()?;
                // Grab the scope at the given depth, and find the start label for it.
                let label = scope_stack[scope_stack.len() - depth as usize - 1].label;
                prg.push(Op::Br(label));
            }
            OpCode::BrIf => {
                let depth = reader.load_imm_varuint32()?;
                let label = scope_stack[scope_stack.len() - depth as usize - 1].label;
                prg.push(Op::BrIf(label));
            }
            OpCode::BrTable => {
                let depth_table = reader.load_array_varu32()?;
                let default = reader.load_imm_varuint32()?;
                let default = scope_stack[scope_stack.len() - default as usize - 1].label;
                let labels_table = depth_table
                    .iter()
                    .map(|depth| scope_stack[scope_stack.len() - *depth as usize - 1].label)
                    .collect();
                prg.push(Op::BrTable(labels_table, default));
            }
            OpCode::Return => {
                prg.push(Op::Return);
            }
            OpCode::Call => {}
            OpCode::CallIndirect => {}
            OpCode::Drop => {
                prg.push(Op::Drop);
            }
            OpCode::Select => {
                prg.push(Op::Select);
            }
            OpCode::GetLocal => {
                let index = reader.load_imm_varuint32()?;
                prg.push(Op::GetLocal(index));
            }
            OpCode::SetLocal => {
                let index = reader.load_imm_varuint32()?;
                prg.push(Op::SetLocal(index));
            }
            OpCode::Tee => {
                let index = reader.load_imm_varuint32()?;
                prg.push(Op::TeeLocal(index));
            }
            OpCode::GetGlobal => {
                let index = reader.load_imm_varuint32()?;
                prg.push(Op::GetGlobal(index));
            }
            OpCode::LoadI32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::LoadI32(memarg));
            }
            OpCode::LoadI64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::LoadI64(memarg));
            }
            OpCode::LoadF32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::LoadF32(memarg));
            }
            OpCode::LoadF64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::LoadF64(memarg));
            }
            OpCode::Load8Se => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load8SE(memarg));
            }

            // Extending load signed
            OpCode::Load16Se => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load16Se(memarg));
            }
            OpCode::Load8I64Se => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load8I64Se(memarg));
            }
            OpCode::Load8I64Ze => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load8I64Ze(memarg));
            }
            OpCode::Load16I64Se => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load16I64Se(memarg));
            }
            OpCode::Load32I64Se => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load32I64Se(memarg));
            }

            // Extending load, unsigned
            OpCode::Load8Ze => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load8Ze(memarg));
            }
            OpCode::Load16Ze => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load16Ze(memarg));
            }
            OpCode::Load16I64Ze => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load16I64Ze(memarg));
            }
            OpCode::Load32I64Ze => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Load32I64Ze(memarg));
            }

            OpCode::StoreI32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::StoreI32(memarg));
            }
            OpCode::StoreI64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::StoreI64(memarg));
            }
            OpCode::StoreF32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::StoreF32(memarg));
            }
            OpCode::StoreF64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::StoreF64(memarg));
            }
            OpCode::Store8_32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Store8_32(memarg));
            }
            OpCode::Store16_32 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Store16_32(memarg));
            }
            OpCode::Store8_64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Store8_64(memarg));
            }
            OpCode::Store16_64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Store16_64(memarg));
            }
            OpCode::Store32_64 => {
                let memarg = read_memarg(&mut reader)?;
                prg.push(Op::Store32_64(memarg));
            }
            OpCode::CurrentMemorySize => {
                prg.push(Op::MemorySize);
            }
            OpCode::GrowMemory => {
                prg.push(Op::MemoryGrow);
            }
            OpCode::I32Const => {
                let value = reader.load_imm_varint32()?;
                prg.push(Op::I32Const(value));
            }
            OpCode::I64Const => {
                let value = reader.load_imm_varint64()?;
                prg.push(Op::I64Const(value));
            }
            OpCode::F32Const => {
                let value = reader.load_imm_f32()?;
                prg.push(Op::F32Const(value));
            }
            OpCode::F64Const => {
                let value = reader.load_imm_f64();
                prg.push(Op::F64Const(value));
            }
            OpCode::I32Eqz => {
                prg.push(Op::I32Eqz);
            }
            OpCode::I32Eq => {
                prg.push(Op::I32Eq);
            }
            OpCode::I32Ne => {
                prg.push(Op::I32Ne);
            }
            OpCode::I32LtS => {
                prg.push(Op::I32LtS);
            }
            OpCode::I32LtU => {
                prg.push(Op::I32LtU);
            }
            OpCode::I32GtS => {
                prg.push(Op::I32GtS);
            }
            OpCode::I32GtU => {
                prg.push(Op::I32GtU);
            }
            OpCode::I32LeS => {
                prg.push(Op::I32LeS);
            }
            OpCode::I32LeU => {
                prg.push(Op::I32LeU);
            }
            OpCode::I32GeS => {
                prg.push(Op::I32GeS);
            }
            OpCode::I32GeU => {
                prg.push(Op::I32GeU);
            }
            OpCode::I64Eqz => {
                prg.push(Op::I64Eqz);
            }
            OpCode::I64Eq => {
                prg.push(Op::I64Eq);
            }
            OpCode::I64Ne => {
                prg.push(Op::I64Ne);
            }
            OpCode::I64LtS => {
                prg.push(Op::I64LtS);
            }
            OpCode::I64LtU => {
                prg.push(Op::I64LtU);
            }
            OpCode::I64GtS => {
                prg.push(Op::I64GtS);
            }
            OpCode::I64GtU => {
                prg.push(Op::I64GtU);
            }
            OpCode::I64LeS => {
                prg.push(Op::I64LeS);
            }
            OpCode::I64LeU => {
                prg.push(Op::I64LeU);
            }
            OpCode::I64GeS => {
                prg.push(Op::I64GeS);
            }
            OpCode::I64GeU => {
                prg.push(Op::I64GeU);
            }
            OpCode::F32Eq => {
                prg.push(Op::F32Eq);
            }
            OpCode::F32Ne => {
                prg.push(Op::F32Ne);
            }
            OpCode::F32Lt => {
                prg.push(Op::F32Lt);
            }
            OpCode::F32Gt => {
                prg.push(Op::F32Gt);
            }
            OpCode::F32Le => {
                prg.push(Op::F32Le);
            }
            OpCode::F32Ge => {
                prg.push(Op::F32Ge);
            }
            OpCode::F64Eq => {
                prg.push(Op::F64Eq);
            }
            OpCode::F64Ne => {
                prg.push(Op::F64Ne);
            }
            OpCode::F64Lt => {
                prg.push(Op::F64Lt);
            }
            OpCode::F64Gt => {
                prg.push(Op::F64Gt);
            }
            OpCode::F64Le => {
                prg.push(Op::F64Le);
            }
            OpCode::F64Ge => {
                prg.push(Op::F64Ge);
            }
            OpCode::I32Clz => {
                prg.push(Op::I32Clz);
            }
            OpCode::I32Ctz => {
                prg.push(Op::I32Ctz);
            }
            OpCode::I32Popcnt => {
                prg.push(Op::I32Popcnt);
            }
            OpCode::I32Add => {
                prg.push(Op::I32Add);
            }
            OpCode::I32Sub => {
                prg.push(Op::I32Sub);
            }
            OpCode::I32Mul => {
                prg.push(Op::I32Mul);
            }
            OpCode::I32DivS => {
                prg.push(Op::I32DivS);
            }
            OpCode::I32DivU => {
                prg.push(Op::I32DivU);
            }
            OpCode::I32RemS => {
                prg.push(Op::I32RemS);
            }
            OpCode::I32RemU => {
                prg.push(Op::I32RemU);
            }
            OpCode::I32And => {
                prg.push(Op::I32And);
            }
            OpCode::I32Or => {
                prg.push(Op::I32Or);
            }
            OpCode::I32Xor => {
                prg.push(Op::I32Xor);
            }
            OpCode::I32Shl => {
                prg.push(Op::I32Shl);
            }
            OpCode::I32ShrS => {
                prg.push(Op::I32ShrS);
            }
            OpCode::I32ShrU => {
                prg.push(Op::I32ShrU);
            }
            OpCode::I32Rotl => {
                prg.push(Op::I32Rotl);
            }
            OpCode::I32Rotr => {
                prg.push(Op::I32Rotr);
            }
            OpCode::I64Clz => {
                prg.push(Op::I64Clz);
            }
            OpCode::I64Ctz => {
                prg.push(Op::I64Ctz);
            }
            OpCode::I64Popcnt => {
                prg.push(Op::I64Popcnt);
            }
            OpCode::I64Add => {
                prg.push(Op::I64Add);
            }
            OpCode::I64Sub => {
                prg.push(Op::I64Sub);
            }
            OpCode::I64Mul => {
                prg.push(Op::I64Mul);
            }
            OpCode::I64DivS => {
                prg.push(Op::I64DivS);
            }
            OpCode::I64DivU => {
                prg.push(Op::I64DivU);
            }
            OpCode::I64RemS => {
                prg.push(Op::I64RemS);
            }
            OpCode::I64RemU => {
                prg.push(Op::I64RemU);
            }
            OpCode::I64And => {
                prg.push(Op::I64And);
            }
            OpCode::I64Or => {
                prg.push(Op::I64Or);
            }
            OpCode::I64Xor => {
                prg.push(Op::I64Xor);
            }
            OpCode::I64Shl => {
                prg.push(Op::I64Shl);
            }
            OpCode::I64ShrS => {
                prg.push(Op::I64ShrS);
            }
            OpCode::I64ShrU => {
                prg.push(Op::I64ShrU);
            }
            OpCode::I64Rotl => {
                prg.push(Op::I64Rotl);
            }
            OpCode::I64Rotr => {
                prg.push(Op::I64Rotr);
            }
            OpCode::F32Abs => {
                prg.push(Op::F32Abs);
            }
            OpCode::F32Neg => {
                prg.push(Op::F32Neg);
            }
            OpCode::F32Ceil => {
                prg.push(Op::F32Ceil);
            }
            OpCode::F32Floor => {
                prg.push(Op::F32Floor);
            }
            OpCode::F32Trunc => {
                prg.push(Op::F32Trunc);
            }
            OpCode::F32Nearest => {
                prg.push(Op::F32Nearest);
            }
            OpCode::F32Sqrt => {
                prg.push(Op::F32Sqrt);
            }
            OpCode::F32Add => {
                prg.push(Op::F32Add);
            }
            OpCode::F32Sub => {
                prg.push(Op::F32Sub);
            }
            OpCode::F32Mul => {
                prg.push(Op::F32Mul);
            }
            OpCode::F32Div => {
                prg.push(Op::F32Div);
            }
            OpCode::F32Min => {
                prg.push(Op::F32Min);
            }
            OpCode::F32Max => {
                prg.push(Op::F32Max);
            }
            OpCode::F32Copysign => {
                prg.push(Op::F32Copysign);
            }
            OpCode::I32WrapI64 => {
                prg.push(Op::I32WrapI64);
            }
            OpCode::I32TruncF32S => {
                prg.push(Op::I32TruncF32S);
            }
            OpCode::I32TruncF32U => {
                prg.push(Op::I32TruncF32U);
            }
            OpCode::I32TruncF64S => {
                prg.push(Op::I32TruncF64S);
            }
            OpCode::I32TruncF64U => {
                prg.push(Op::I32TruncF64U);
            }
            OpCode::I64ExtendI32S => {
                prg.push(Op::I64ExtendI32S);
            }
            OpCode::I64ExtendI32U => {
                prg.push(Op::I64ExtendI32U);
            }
            OpCode::I64TruncF32S => {
                prg.push(Op::I64TruncF32S);
            }
            OpCode::I64TruncF32U => {
                prg.push(Op::I64TruncF32U);
            }
            OpCode::I64TruncF64S => {
                prg.push(Op::I64TruncF64S);
            }
            OpCode::I64TruncF64U => {
                prg.push(Op::I64TruncF64U);
            }
            OpCode::F32ConvertI32S => {
                prg.push(Op::F32ConvertI32S);
            }
            OpCode::F32ConvertI32U => {
                prg.push(Op::F32ConvertI32U);
            }
            OpCode::F32ConvertI64S => {
                prg.push(Op::F32ConvertI64S);
            }
            OpCode::F32ConvertI64U => {
                prg.push(Op::F32ConvertI64U);
            }
            OpCode::F32DemoteF64 => {
                prg.push(Op::F32DemoteF64);
            }
            OpCode::F64ConvertI32S => {
                prg.push(Op::F64ConvertI32S);
            }
            OpCode::F64ConvertI32U => {
                prg.push(Op::F64ConvertI32U);
            }
            OpCode::F64ConvertI64S => {
                prg.push(Op::F64ConvertI64S);
            }
            OpCode::F64ConvertI64U => {
                prg.push(Op::F64ConvertI64U);
            }
            OpCode::F64PromoteF32 => {
                prg.push(Op::F64PromoteF32);
            }
            OpCode::I32ReinterpretF32 => {
                prg.push(Op::I32ReinterpretF32);
            }
            OpCode::I64ReinterpretF64 => {
                prg.push(Op::I64ReinterpretF64);
            }
            OpCode::F32ReinterpretI32 => {
                prg.push(Op::F32ReinterpretI32);
            }
            OpCode::F64ReinterpretI64 => {
                prg.push(Op::F64ReinterpretI64);
            }
            OpCode::I32Extend8S => {
                prg.push(Op::I32Extend8S);
            }
            OpCode::I32Extend16S => {
                prg.push(Op::I32Extend16S);
            }
            OpCode::I64Extend8S => {
                prg.push(Op::I64Extend8S);
            }
            OpCode::I64Extend16S => {
                prg.push(Op::I64Extend16S);
            }
            OpCode::I64Extend32S => {
                prg.push(Op::I64Extend32S);
            }

            OpCode::SelectT
            | OpCode::TableGet
            | OpCode::TableSet
            | OpCode::RefNull
            | OpCode::IsNull
            | OpCode::RefFunc
            | OpCode::RefAsNonNull
            | OpCode::RefEq => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Reference types proposal not supported".to_string(),
                ));
            }

            OpCode::Try | OpCode::Catch | OpCode::Throw | OpCode::Rethrow | OpCode::ThrowRef => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Exceptions proposal not supported".to_string(),
                ));
            }

            OpCode::ReturnCall
            | OpCode::ReturnCallIndirect
            | OpCode::CallRef
            | OpCode::ReturnCallRef => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Typed function references proposal not supported".to_string(),
                ));
            }

            OpCode::Delegate | OpCode::CatchAll | OpCode::TryTable => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Exception handling proposal not supported".to_string(),
                ));
            }
            OpCode::BrOnNull | OpCode::BrOnNonNull => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Reference types proposal not supported".to_string(),
                ));
            }
            OpCode::FCExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Function call extension proposal not supported".to_string(),
                ));
            }
            OpCode::SIMDExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "SIMD proposal not supported".to_string(),
                ));
            }
            OpCode::GCExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Garbage collection proposal not supported".to_string(),
                ));
            }
            OpCode::ThreadsExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Threads proposal not supported".to_string(),
                ));
            }
        }
    }

    Ok(prg)
}

/// Scan instructions but don't decode. Used for loading expressions in modules.
pub fn scan(reader: &mut LEB128Reader) -> Result<usize, DecodeError> {
    let mut scope_stack = vec![];

    // Decode the raw program stream and translate it into our ADT Op
    while reader.remaining() != 0 {
        let opcode_o = reader.load_imm_u8()?;
        let opcode: OpCode =
            OpCode::from_repr(opcode_o).ok_or(DecodeError::InvalidOpcode(opcode_o))?;

        // Block-stack... we need to keep track of the current block we're while decoding.
        match opcode {
            OpCode::Block | OpCode::Loop | OpCode::If => {
                reader.load_imm_u8()?;
                scope_stack.push(opcode);
            }
            OpCode::Else => {
                if scope_stack.is_empty() {
                    return Err(DecodeError::FailedToDecode("Else without If".to_string()));
                }
            }
            OpCode::End => {
                if scope_stack.is_empty() {
                    break;
                }
                let Some(_) = scope_stack.pop() else {
                    return Err(DecodeError::FailedToDecode("End without block".to_string()));
                };
            }
            OpCode::Br | OpCode::BrIf => {
                reader.load_imm_varuint32()?;
            }
            OpCode::BrTable => {
                reader.load_array_varu32()?;
                reader.load_imm_varuint32()?;
            }
            OpCode::GetLocal | OpCode::SetLocal | OpCode::Tee | OpCode::GetGlobal => {
                reader.load_imm_varuint32()?;
            }
            OpCode::LoadI32
            | OpCode::LoadI64
            | OpCode::LoadF32
            | OpCode::LoadF64
            | OpCode::Load8Se
            | OpCode::Load16Se
            | OpCode::Load8I64Se
            | OpCode::Load8I64Ze
            | OpCode::Load16I64Se
            | OpCode::Load32I64Se
            | OpCode::Load8Ze
            | OpCode::Load16Ze
            | OpCode::Load16I64Ze
            | OpCode::Load32I64Ze
            | OpCode::StoreI32
            | OpCode::StoreI64
            | OpCode::StoreF32
            | OpCode::StoreF64
            | OpCode::Store8_32
            | OpCode::Store16_32
            | OpCode::Store8_64
            | OpCode::Store16_64
            | OpCode::Store32_64 => {
                read_memarg(reader)?;
            }
            OpCode::I32Const => {
                reader.load_imm_varint32()?;
            }
            OpCode::I64Const => {
                reader.load_imm_varint64()?;
            }
            OpCode::F32Const => {
                reader.load_imm_f32()?;
            }
            OpCode::F64Const => {
                reader.load_imm_f64();
            }

            OpCode::SelectT
            | OpCode::TableGet
            | OpCode::TableSet
            | OpCode::RefNull
            | OpCode::IsNull
            | OpCode::RefFunc
            | OpCode::RefAsNonNull
            | OpCode::RefEq => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Reference types proposal not supported".to_string(),
                ));
            }

            OpCode::I32Extend8S
            | OpCode::I32Extend16S
            | OpCode::I64Extend8S
            | OpCode::I64Extend16S
            | OpCode::I64Extend32S => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Sign-extension operators proposal not supported".to_string(),
                ));
            }

            OpCode::Try => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Exceptions proposal not supported".to_string(),
                ));
            }

            OpCode::ReturnCall
            | OpCode::ReturnCallIndirect
            | OpCode::CallRef
            | OpCode::ReturnCallRef => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Typed function references proposal not supported".to_string(),
                ));
            }

            OpCode::Delegate | OpCode::CatchAll | OpCode::TryTable => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Exception handling proposal not supported".to_string(),
                ));
            }
            OpCode::BrOnNull | OpCode::BrOnNonNull => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Reference types proposal not supported".to_string(),
                ));
            }
            OpCode::FCExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Function call extension proposal not supported".to_string(),
                ));
            }
            OpCode::SIMDExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "SIMD proposal not supported".to_string(),
                ));
            }
            OpCode::GCExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Garbage collection proposal not supported".to_string(),
                ));
            }
            OpCode::ThreadsExtension => {
                return Err(DecodeError::UnimplementedOpcode(
                    opcode_o,
                    "Threads proposal not supported".to_string(),
                ));
            }
            _ => continue,
        }
    }

    Ok(reader.position())
}
