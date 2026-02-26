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
pub mod bench_suite;
mod exec;
mod frame;
mod import;
mod instance;
mod memory;
mod module;
mod op;
mod opcode;
mod stack;

pub use crate::decode::DecodeError;
use crate::module::LEB128Reader;
pub use exec::{Continuation, ExecError, ExecutionState, RunResult, Value};
pub use frame::Frame;
pub use import::{HostFunction, ImportResolver, ImportedFunction};
pub use instance::LinkError;
pub use instance::{mk_instance, mk_instance_with_imports, Instance, TableInstance};
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
            0x73 => Ok(ValueType::FuncRef),
            0x6f => Ok(ValueType::ExternRef),
            0x6e | 0x6d | 0x6c | 0x6b | 0x6a | 0x69 | 0x68 | 0x71 | 0x72 | 0x74 | 0x75 => {
                Ok(ValueType::ExternRef)
            }
            0x7F => Ok(ValueType::I32),
            0x7E => Ok(ValueType::I64),
            0x7D => Ok(ValueType::F32),
            0x7C => Ok(ValueType::F64),
            0x7B => Ok(ValueType::V128),
            _ => Err(DecodeError::InvalidSignature(value)),
        }
    }

    fn from_s33(value: i32) -> Result<Self, DecodeError> {
        match value {
            -64 => Ok(ValueType::Unit),
            -1 => Ok(ValueType::I32),
            -2 => Ok(ValueType::I64),
            -3 => Ok(ValueType::F32),
            -4 => Ok(ValueType::F64),
            -5 => Ok(ValueType::V128),
            -16 => Ok(ValueType::FuncRef),
            -13 => Ok(ValueType::FuncRef),
            -17 => Ok(ValueType::ExternRef),
            // Accept newer abstract heap/reference families as a generic host reference.
            -18 | -19 | -20 | -21 | -22 | -23 | -24 | -15 | -14 | -12 | -11 => {
                Ok(ValueType::ExternRef)
            }
            _ => Err(DecodeError::InvalidSignature(value as u32)),
        }
    }

    fn read_ref_type_suffix(reader: &mut LEB128Reader) -> Result<Self, DecodeError> {
        let heap_type = reader.load_imm_varint32()?;
        match heap_type {
            -17 => Ok(ValueType::ExternRef),
            -13 | -16 => Ok(ValueType::FuncRef),
            // Treat concrete and GC heap types as generic references for now.
            _ if heap_type >= 0 => Ok(ValueType::FuncRef),
            _ => Ok(ValueType::ExternRef),
        }
    }

    fn try_read_with_lead(reader: &mut LEB128Reader, lead: u8) -> Result<Self, DecodeError> {
        match lead {
            0x63 | 0x64 | 0x65 => Self::read_ref_type_suffix(reader),
            _ => Self::from_u32(lead as u32),
        }
    }

    /// Read a value type from a reader without allowing for type index indirection.
    fn read(reader: &mut LEB128Reader) -> Result<Self, DecodeError> {
        let lead = reader.load_imm_u8()?;
        Self::try_read_with_lead(reader, lead)
    }

    /// Read a full signature, allowing for type index indirection.
    fn read_signature(reader: &mut LEB128Reader) -> Result<TypeSignature, DecodeError> {
        let lead = reader.load_imm_u8()?;
        if lead == 0x40 {
            return Ok(TypeSignature::ValueType(ValueType::Unit));
        }

        // Common case: direct value/ref type encoding.
        if let Ok(vt) = Self::try_read_with_lead(reader, lead) {
            return Ok(TypeSignature::ValueType(vt));
        }

        // Otherwise this is a signed LEB block type index that started with `lead`.
        let mut value = (lead & 0x7f) as i32;
        let mut shift = 7;
        let mut last_byte = lead;
        while (last_byte & 0x80) != 0 {
            let byte = reader.load_imm_u8()?;
            value |= ((byte & 0x7f) as i32) << shift;
            shift += 7;
            last_byte = byte;
        }
        if shift < 32 && (last_byte & 0x40) != 0 {
            value |= !0 << shift;
        }

        if let Ok(vt) = Self::from_s33(value) {
            return Ok(TypeSignature::ValueType(vt));
        }
        if value < 0 {
            return Err(DecodeError::InvalidSignature(value as u32));
        }
        Ok(TypeSignature::Index(value as u32))
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}
