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

//! A minimal dependency, minimal feature WASM interpreter meant to be used embedded in other
//! runtimes.
//! Will not support WASI or anything fancy, just the bare minimum to run a WASM module without
//! POSIX or anything similar.
//! Aim is not performance, but to allow the following:
//!     Extremely easy to embed, and no futzing with lifetimes or ownership
//!     Should ideally support a no_std environment
//!     Main opcode interpreter can be externally driven on a tick slice
//!     Execution can be stopped and restarted
//!     Entire engine / stack is both `Send` and serializable/deserializable
//!     No SIMD, no Threads, no exceptions proposal, no tail call proposal
//!          MAYBE GC proposal, but not sure yet

mod decode;
mod exec;
mod frame;
mod instance;
mod memory;
mod module;
mod op;
mod opcode;
mod stack;

pub use crate::decode::DecodeError;
use crate::module::LEB128Reader;
pub use exec::{ExecError, Execution, Value};
pub use frame::Frame;
pub use instance::LinkError;
pub use instance::{mk_instance, Instance, TableInstance};
pub use memory::{Memory, SliceMemory, VectorMemory};
pub use module::{
    Code, Data, ElementMode, ElementSegment, Elements, Global, ImportExportKind, LoaderError,
    MemorySection, Module, ReferenceType, SectionInfo,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    ValueType(ValueType),
    FunctionType(FuncType),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FuncType {
    pub params: Vec<ValueType>,
    pub results: Vec<ValueType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Unit,
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeSignature {
    ValueType(ValueType),
    Index(u32),
}

impl ValueType {
    fn from_u32(value: u32) -> Result<Self, DecodeError> {
        match value {
            0x40 => Ok(ValueType::Unit),
            0x70 => Ok(ValueType::FuncRef),
            0x6f => Ok(ValueType::ExternRef),
            0x7F => Ok(ValueType::I32),
            0x7E => Ok(ValueType::I64),
            0x7D => Ok(ValueType::F32),
            0x7C => Ok(ValueType::F64),
            0x7B => Ok(ValueType::V128),
            _ => Err(DecodeError::InvalidSignature(value)),
        }
    }

    /// Read a value type from a reader without allowing for type index indirection.
    fn read(reader: &mut LEB128Reader) -> Result<Self, DecodeError> {
        let value = reader.load_imm_varuint32()?;
        if value == 0x40 {
            return Ok(ValueType::Unit);
        }
        Self::from_u32(value)
    }

    /// Read a full signature, allowing for type index indirection.
    fn read_signature(reader: &mut LEB128Reader) -> Result<TypeSignature, DecodeError> {
        let value = reader.load_imm_varint32()?;
        if value == 0x40 {
            return Ok(TypeSignature::ValueType(ValueType::Unit));
        }
        if value < 0 {
            return Self::from_u32(value.unsigned_abs()).map(TypeSignature::ValueType);
        }
        // Also check if it's a value type when interpreted as unsigned
        if let Ok(vt) = Self::from_u32(value as u32) {
            return Ok(TypeSignature::ValueType(vt));
        }
        Ok(TypeSignature::Index(value as u32))
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}
