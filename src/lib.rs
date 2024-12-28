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
//!     No SIMD, no Threads, No reference types proposal, no exceptions proposal, no tail call proposal
//!          MAYBE GC proposal, but not sure yet

mod decode;
mod exec;
mod frame;
mod link;
mod memory;
mod module;
mod op;
mod opcode;
mod stack;

pub use crate::decode::DecodeError;
use crate::module::LEB128Reader;
pub use frame::Frame;
pub use link::{link, Linked};
pub use memory::SliceMemory;
pub use module::{
    Code, Data, ElementMode, ElementSegment, Elements, Global, ImportExportKind, LoaderError,
    MemorySection, Module, ReferenceType, SectionInfo,
};

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

impl ValueType {
    fn read(reader: &mut LEB128Reader) -> Result<Self, DecodeError> {
        let value = reader.load_imm_varuint32()?;
        match value {
            0x40 => Ok(ValueType::Unit),
            0x70 | 0x6f => Err(DecodeError::UnsupportedType(
                value,
                "Reference types proposal unsupported".to_string(),
            )),
            0x7F => Ok(ValueType::I32),
            0x7E => Ok(ValueType::I64),
            0x7D => Ok(ValueType::F32),
            0x7C => Ok(ValueType::F64),
            0x7B => Ok(ValueType::V128),
            _ => Err(DecodeError::InvalidSignature(value)),
        }
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}
