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

use strum_macros::FromRepr;

/// The raw opcodes in their byte form.
/// (These then get translated into a richer Op enum in the parser phase)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr)]
pub enum OpCode {
    Unreachable = 0x00,
    Nop = 0x01,
    Block = 0x02,
    Loop = 0x03,
    If = 0x04,
    Else = 0x05,

    // Exceptions proposal:
    Try = 0x06,
    Catch = 0x07,
    Throw = 0x08,
    Rethrow = 0x09,
    ThrowRef = 0x0A,

    End = 0x0B,
    Br = 0x0C,
    BrIf = 0x0D,
    BrTable = 0x0E,
    Return = 0x0F,

    // Exception handling proposal
    Delegate = 0x18,
    CatchAll = 0x19,
    TryTable = 0x1D,

    Call = 0x10,
    CallIndirect = 0x11,

    // Tail call proposal:
    ReturnCall = 0x12,
    ReturnCallIndirect = 0x13,
    CallRef = 0x14,
    ReturnCallRef = 0x15,

    Drop = 0x1A,
    Select = 0x1B,

    // Reference types proposal
    SelectT = 0x1C,
    TableGet = 0x25,
    TableSet = 0x26,
    RefNull = 0xD0,
    IsNull = 0xD1,
    RefFunc = 0xD2,
    RefAsNonNull = 0xD3,
    RefEq = 0xD5,

    // Typed function references proposal
    BrOnNull = 0xD4,
    BrOnNonNull = 0xD6,

    GetLocal = 0x20,
    SetLocal = 0x21,
    Tee = 0x22,
    GetGlobal = 0x23,
    SetGlobal = 0x24,

    LoadI32 = 0x28,
    LoadI64 = 0x29,
    LoadF32 = 0x2A,
    LoadF64 = 0x2B,

    // (Extending load signed)
    /// Load byte and sign extend to i32
    Load8Se = 0x2C,
    /// Load short and sign extend to i32
    Load16Se = 0x2E,
    /// Load byte and sign extend to i64
    Load8I64Se = 0x30,
    /// Load short and sign extend to i64
    Load16I64Se = 0x32,
    /// Load int and sign extend to i64
    Load32I64Se = 0x34,

    // (Extending load unsigned)
    // Load byte and zero extend to i32
    Load8Ze = 0x2D,
    /// Load byte and zero extend to i64
    Load8I64Ze = 0x31,
    /// Load int and zero extend to i64
    Load32I64Ze = 0x35,
    /// Load short and zero extend to i32
    Load16Ze = 0x2F,
    /// Load short and zero extend to i64
    Load16I64Ze = 0x33,

    StoreI32 = 0x36,
    StoreI64 = 0x37,
    StoreF32 = 0x38,
    StoreF64 = 0x39,

    // Wrap i32 to i8 and store
    Store8_32 = 0x3A,
    // Wrap i32 to i16 and store
    Store16_32 = 0x3B,
    // Wrap i64 to i8 and store
    Store8_64 = 0x3C,
    // Wrap i64 to i16 and store
    Store16_64 = 0x3D,
    // Wrap i64 to i32 and store
    Store32_64 = 0x3E,

    CurrentMemorySize = 0x3F,
    GrowMemory = 0x40,

    I32Const = 0x41,
    I64Const = 0x42,
    F32Const = 0x43,
    F64Const = 0x44,

    I32Eqz = 0x45,
    I32Eq = 0x46,
    I32Ne = 0x47,
    I32LtS = 0x48,
    I32LtU = 0x49,
    I32GtS = 0x4A,
    I32GtU = 0x4B,
    I32LeS = 0x4C,
    I32LeU = 0x4D,
    I32GeS = 0x4E,
    I32GeU = 0x4F,

    I64Eqz = 0x50,
    I64Eq = 0x51,
    I64Ne = 0x52,
    I64LtS = 0x53,
    I64LtU = 0x54,
    I64GtS = 0x55,
    I64GtU = 0x56,
    I64LeS = 0x57,
    I64LeU = 0x58,
    I64GeS = 0x59,
    I64GeU = 0x5A,

    F32Eq = 0x5B,
    F32Ne = 0x5C,
    F32Lt = 0x5D,
    F32Gt = 0x5E,
    F32Le = 0x5F,
    F32Ge = 0x60,

    F64Eq = 0x61,
    F64Ne = 0x62,
    F64Lt = 0x63,
    F64Gt = 0x64,
    F64Le = 0x65,
    F64Ge = 0x66,

    I32Clz = 0x67,
    I32Ctz = 0x68,
    I32Popcnt = 0x69,
    I32Add = 0x6A,
    I32Sub = 0x6B,
    I32Mul = 0x6C,
    I32DivS = 0x6D,
    I32DivU = 0x6E,
    I32RemS = 0x6F,
    I32RemU = 0x70,
    I32And = 0x71,
    I32Or = 0x72,
    I32Xor = 0x73,
    I32Shl = 0x74,
    I32ShrS = 0x75,
    I32ShrU = 0x76,
    I32Rotl = 0x77,
    I32Rotr = 0x78,

    I64Clz = 0x79,
    I64Ctz = 0x7A,
    I64Popcnt = 0x7B,
    I64Add = 0x7C,
    I64Sub = 0x7D,
    I64Mul = 0x7E,
    I64DivS = 0x7F,
    I64DivU = 0x80,
    I64RemS = 0x81,
    I64RemU = 0x82,
    I64And = 0x83,
    I64Or = 0x84,
    I64Xor = 0x85,
    I64Shl = 0x86,
    I64ShrS = 0x87,
    I64ShrU = 0x88,
    I64Rotl = 0x89,
    I64Rotr = 0x8A,

    F32Abs = 0x8B,
    F32Neg = 0x8C,
    F32Ceil = 0x8D,
    F32Floor = 0x8E,
    F32Trunc = 0x8F,
    F32Nearest = 0x90,
    F32Sqrt = 0x91,
    F32Add = 0x92,
    F32Sub = 0x93,
    F32Mul = 0x94,
    F32Div = 0x95,
    F32Min = 0x96,
    F32Max = 0x97,
    F32Copysign = 0x98,

    F64Abs = 0x99,
    F64Neg = 0x9A,
    F64Ceil = 0x9B,
    F64Floor = 0x9C,
    F64Trunc = 0x9D,
    F64Nearest = 0x9E,
    F64Sqrt = 0x9F,

    F64Add = 0xA0,
    F64Sub = 0xA1,
    F64Mul = 0xA2,
    F64Div = 0xA3,
    F64Min = 0xA4,
    F64Max = 0xA5,
    F64Copysign = 0xA6,
    I32WrapI64 = 0xA7,
    I32TruncF32S = 0xA8,
    I32TruncF32U = 0xA9,
    I32TruncF64S = 0xAA,
    I32TruncF64U = 0xAB,
    I64ExtendI32S = 0xAC,
    I64ExtendI32U = 0xAD,
    I64TruncF32S = 0xAE,
    I64TruncF32U = 0xAF,
    I64TruncF64S = 0xB0,
    I64TruncF64U = 0xB1,

    F32ConvertI32S = 0xB2,
    F32ConvertI32U = 0xB3,
    F32ConvertI64S = 0xB4,
    F32ConvertI64U = 0xB5,
    F32DemoteF64 = 0xB6,
    F64ConvertI32S = 0xB7,
    F64ConvertI32U = 0xB8,
    F64ConvertI64S = 0xB9,
    F64ConvertI64U = 0xBA,
    F64PromoteF32 = 0xBB,

    I32ReinterpretF32 = 0xBC,
    I64ReinterpretF64 = 0xBD,
    F32ReinterpretI32 = 0xBE,
    F64ReinterpretI64 = 0xBF,

    // Sign-extension operators proposal
    I32Extend8S = 0xC0,
    I32Extend16S = 0xC1,
    I64Extend8S = 0xC2,
    I64Extend16S = 0xC3,
    I64Extend32S = 0xC4,

    // GC extension
    GCExtension = 0xFB,
    // FC extension
    FCExtension = 0xFC,
    // SIMD extension
    SIMDExtension = 0xFD,
    // Threads extension
    ThreadsExtension = 0xFE,
}
