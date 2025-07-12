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

use crate::decode::ScopeType;
use crate::TypeSignature;

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct MemArg {
    pub offset: usize,
    pub align: u32,
}

/// A semantically richer, decoded version of all the WASM opcodes.
/// To avoid having varints and having to deal with block structuring issues.
/// The program will take a sequence of raw OpCodes and turn them into this.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Nop,
    Unreachable,

    // Control flow.
    /// Block->End
    StartScope(TypeSignature, ScopeType),
    EndScope(ScopeType),
    /// If with condition check - no labels needed
    If,
    /// Else marker - no labels needed  
    Else,
    Br(u32),
    BrIf(u32),
    BrTable(Vec<u32>, u32),
    Return,

    // Calls
    Call(u32),
    CallIndirect(u32, u32), // (type_idx, table_idx)

    Drop,
    Select,

    // Locals
    GetLocal(u32),
    SetLocal(u32),
    TeeLocal(u32),
    GetGlobal(u32),
    SetGlobal(u32),

    // Table operations
    TableGet(u32),
    TableSet(u32),

    // Loads.
    LoadI32(MemArg),
    LoadI64(MemArg),
    LoadF32(MemArg),
    LoadF64(MemArg),

    // Load byte and sign extend to i32
    Load8SE(MemArg),
    // Load byte and zero extend to i32
    Load8Ze(MemArg),

    // Load short and sign extend to i32
    Load16Se(MemArg),
    // Load short and zero extend to i32
    Load16Ze(MemArg),

    // Load byte and sign extend to i64
    Load8I64Se(MemArg),
    // Load byte and zero extend to i64
    Load8I64Ze(MemArg),

    // Load short and sign extend to i64
    Load16I64Se(MemArg),
    // Load short and zero extend to i64
    Load16I64Ze(MemArg),

    // Load int and sign extend to i64
    Load32I64Se(MemArg),
    // Load int and zero extend to i64
    Load32I64Ze(MemArg),

    // Stores. Same deal.
    StoreI32(MemArg),
    StoreI64(MemArg),
    StoreF32(MemArg),
    StoreF64(MemArg),

    // Wrap i32 to i8 and store
    Store8_32(MemArg),
    // Wrap i32 to i16 and store
    Store16_32(MemArg),
    // Wrap i64 to i8 and store
    Store8_64(MemArg),
    // Wrap i64 to i16 and store
    Store16_64(MemArg),
    // Wrap i64 to i32 and store
    Store32_64(MemArg),

    // Constants
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),

    // Memory
    MemorySize,
    MemoryGrow,

    // The remainder are all operations which operate purely off the stack and are 1:1 with their
    // raw opcode counterparts.
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,

    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,

    F32Eq,
    F32Ne,
    F32Lt,
    F32Gt,
    F32Le,
    F32Ge,

    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,

    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,

    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,

    F32Abs,
    F32Neg,
    F32Ceil,
    F32Floor,
    F32Trunc,
    F32Nearest,
    F32Sqrt,
    F32Add,
    F32Sub,
    F32Mul,
    F32Div,
    F32Min,
    F32Max,
    F32Copysign,

    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Min,
    F64Max,
    F64Copysign,

    F64Abs,
    F64Neg,
    F64Ceil,
    F64Floor,
    F64Trunc,
    F64Nearest,
    F64Sqrt,

    I32WrapI64,
    I32TruncF32S,
    I32TruncF32U,
    I32TruncF64S,
    I32TruncF64U,
    I64ExtendI32S,
    I64ExtendI32U,
    I64TruncF32S,
    I64TruncF32U,
    I64TruncF64S,
    I64TruncF64U,

    // Nontrapping float-to-int conversions (FC extension)
    I32TruncSatF32S,
    I32TruncSatF32U,
    I32TruncSatF64S,
    I32TruncSatF64U,
    I64TruncSatF32S,
    I64TruncSatF32U,
    I64TruncSatF64S,
    I64TruncSatF64U,

    F32ConvertI32S,
    F32ConvertI32U,
    F32ConvertI64S,
    F32ConvertI64U,
    F32DemoteF64,
    F64ConvertI32S,
    F64ConvertI32U,
    F64ConvertI64S,
    F64ConvertI64U,
    F64PromoteF32,

    I32ReinterpretF32,
    I64ReinterpretF64,
    F32ReinterpretI32,
    F64ReinterpretI64,

    // Sign-extension operators proposal
    I32Extend8S,
    I32Extend16S,
    I64Extend8S,
    I64Extend16S,
    I64Extend32S,

    // Reference types proposal
    RefNull(crate::ValueType),
    RefFunc(u32),
    RefIsNull,
    RefAsNonNull,
    RefEq,
    SelectT(Vec<crate::ValueType>),
}
