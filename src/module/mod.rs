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

mod leb128;
mod parse;

pub use crate::module::leb128::LEB128Reader;
use crate::module::parse::{
    SECTION_ID_CODE, SECTION_ID_CUSTOM, SECTION_ID_DATA, SECTION_ID_DATA_COUNT, SECTION_ID_ELEMENT,
    SECTION_ID_EXPORT, SECTION_ID_FUNCTION, SECTION_ID_GLOBAL, SECTION_ID_IMPORT,
    SECTION_ID_MEMORY, SECTION_ID_START, SECTION_ID_TABLE, SECTION_ID_TYPE,
};
use crate::LoaderError::{DecoderError, UnsupportedSectionType};
use crate::{DecodeError, FuncType, ValueType};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum LoaderError {
    InvalidMagicNumber,
    InvalidVersion,
    InvalidSectionType(u8),
    InvalidImportType(u8),
    InvalidReferenceType(u8),
    InvalidInstruction,
    MismatchedBlockStack,
    UnsupportedSectionType(SectionType),
    UnsupportedElementSegment(u8),
    DecoderError(DecodeError),
}

impl Display for LoaderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderError::InvalidMagicNumber => write!(f, "Invalid magic number"),
            LoaderError::InvalidVersion => write!(f, "Invalid version"),
            LoaderError::InvalidSectionType(t) => write!(f, "Invalid section type: {t}"),
            UnsupportedSectionType(t) => {
                write!(f, "Unsupported section type: {t:?}")
            }
            LoaderError::UnsupportedElementSegment(k) => {
                write!(f, "Unsupported element kind: {k}")
            }
            LoaderError::InvalidInstruction => write!(f, "Invalid instruction"),
            LoaderError::MismatchedBlockStack => write!(f, "Mismatched block stack"),
            LoaderError::InvalidReferenceType(t) => write!(f, "Invalid reference type: {t}"),
            LoaderError::InvalidImportType(t) => write!(f, "Invalid import type: {t}"),
            DecoderError(e) => write!(f, "Decode error: {e}"),
        }
    }
}

impl Error for LoaderError {}

#[repr(u8)]
#[derive(Debug, Copy, Clone)]
pub enum SectionType {
    Custom = SECTION_ID_CUSTOM,
    Type = SECTION_ID_TYPE,
    Import = SECTION_ID_IMPORT,
    Function = SECTION_ID_FUNCTION,
    Table = SECTION_ID_TABLE,
    Memory = SECTION_ID_MEMORY,
    Global = SECTION_ID_GLOBAL,
    Export = SECTION_ID_EXPORT,
    Start = SECTION_ID_START,
    Element = SECTION_ID_ELEMENT,
    Code = SECTION_ID_CODE,
    Data = SECTION_ID_DATA,
    DataCount = SECTION_ID_DATA_COUNT,
}

impl SectionType {
    pub fn from_u8(value: u8) -> Result<Self, LoaderError> {
        match value {
            SECTION_ID_CUSTOM => Ok(SectionType::Custom),
            SECTION_ID_TYPE => Ok(SectionType::Type),
            SECTION_ID_IMPORT => Ok(SectionType::Import),
            SECTION_ID_FUNCTION => Ok(SectionType::Function),
            SECTION_ID_TABLE => Ok(SectionType::Table),
            SECTION_ID_MEMORY => Ok(SectionType::Memory),
            SECTION_ID_GLOBAL => Ok(SectionType::Global),
            SECTION_ID_EXPORT => Ok(SectionType::Export),
            SECTION_ID_START => Ok(SectionType::Start),
            SECTION_ID_ELEMENT => Ok(SectionType::Element),
            SECTION_ID_CODE => Ok(SectionType::Code),
            SECTION_ID_DATA => Ok(SectionType::Data),
            SECTION_ID_DATA_COUNT => Ok(SectionType::DataCount),
            _ => Err(LoaderError::InvalidSectionType(value)),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ReferenceType {
    FuncRef = 0x70,
    ExternRef = 0x6f,
}

impl ReferenceType {
    pub fn from_u8(value: u8) -> Result<Self, LoaderError> {
        match value {
            0x70 => Ok(ReferenceType::FuncRef),
            0x6f => Ok(ReferenceType::ExternRef),
            _ => Err(LoaderError::InvalidReferenceType(value)),
        }
    }
}

pub struct SectionInfo {
    pub id: u8,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportEntry {
    // TODO: This could be offsets instead of copying...
    pub(crate) name: String,
    pub(crate) kind: ImportExportKind,
    pub(crate) index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub ty: ReferenceType,
    pub limits: (u32, Option<u32>),
}

/// Declaration of a memory section in the program.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySection {
    /// Min pages, optional max pages.
    pub limits: (u32, Option<u32>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Active {
        expr: Region,
        data: Region,
    },
    Passive {
        data: Region,
    },
    ActiveMemIdx {
        memidx: u32,
        expr: Region,
        data: Region,
    },
}

#[derive(Debug)]
pub struct Code {
    pub locals: Vec<ValueType>,
    pub code: Region,
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ImportExportKind {
    Function = 0x00,
    Table = 0x01,
    Memory = 0x02,
    Global = 0x03,
    // Tag = 0x04,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Import {
    Func(u32),
    Table(ReferenceType, (u32, Option<u32>)),
    Memory((u32, Option<u32>)),
    Global(ValueType, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Global {
    pub ty: ValueType,
    pub mutable: bool,
    pub expr: Region,
}

#[derive(Debug)]
pub enum ElementMode {
    Passive,
    Active { table_index: u32, expr: Region },
    Declarative,
}

#[derive(Debug)]
pub enum Elements {
    Function(Vec<u32>),
    Expression(Vec<Region>),
}

#[derive(Debug)]
pub struct ElementSegment {
    pub reftype: ReferenceType,
    pub elements: Elements,
    pub mode: ElementMode,
}

impl ImportExportKind {
    pub fn from_u8(value: u8) -> Result<Self, LoaderError> {
        match value {
            0x00 => Ok(ImportExportKind::Function),
            0x01 => Ok(ImportExportKind::Table),
            0x02 => Ok(ImportExportKind::Memory),
            0x03 => Ok(ImportExportKind::Global),
            _ => Err(LoaderError::InvalidImportType(value)),
        }
    }
}

/// Represents a WASM binary, loaded.
/// Holds not just the program, but parsed data about the program such as its block structure,
/// number of locals, etc.
pub struct Module {
    // The original unmolested binary format.
    pub module_data: Vec<u8>,
    pub version: u32,
    pub types: Vec<FuncType>,
    pub code: Vec<Code>,
    pub tables: Vec<Table>,
    pub functions: Vec<usize>,
    pub exports: Vec<ExportEntry>,
    pub imports: Vec<(String, String, Import)>,
    pub memories: Vec<MemorySection>,
    pub globals: Vec<Global>,
    pub data: Vec<Data>,
    pub start_function: Option<usize>,
    pub element_segments: Vec<ElementSegment>,
}

impl Module {
    pub fn code(&self, index: usize) -> &[u8] {
        let code = &self.code[index];
        let (start, end) = code.code;

        (&self.module_data[start..end]) as _
    }
}

pub type Region = (usize, usize);

impl Module {
    pub fn get_expr(&self, region: &Region) -> &[u8] {
        assert_eq!(self.module_data[region.1], 0x0b);
        &self.module_data[region.0..region.1]
    }
}
