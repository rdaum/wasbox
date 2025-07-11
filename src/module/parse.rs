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

use crate::module::leb128::LEB128Reader;
use crate::module::{
    Code, Data, ElementMode, ElementSegment, Elements, ExportEntry, Import, ImportExportKind,
    MemorySection, ReferenceType, Region, SectionType, Table,
};
use crate::DecodeError::{FailedToDecode, InvalidDataSegmentType, MalformedMemory};
use crate::LoaderError::DecoderError;
use crate::{DecodeError, FuncType, Global, LoaderError, Module, ValueType};

pub const SECTION_ID_CUSTOM: u8 = 0;
pub const SECTION_ID_TYPE: u8 = 1;
pub const SECTION_ID_IMPORT: u8 = 2;
pub const SECTION_ID_FUNCTION: u8 = 3;
pub const SECTION_ID_TABLE: u8 = 4;
pub const SECTION_ID_MEMORY: u8 = 5;
pub const SECTION_ID_GLOBAL: u8 = 6;
pub const SECTION_ID_EXPORT: u8 = 7;
pub const SECTION_ID_START: u8 = 8;
pub const SECTION_ID_ELEMENT: u8 = 9;
pub const SECTION_ID_CODE: u8 = 10;
pub const SECTION_ID_DATA: u8 = 11;
pub const SECTION_ID_DATA_COUNT: u8 = 12;

fn read_limits(reader: &mut LEB128Reader) -> Result<(u32, Option<u32>), DecodeError> {
    // "Limits are encoded with a preceding flag indicating whether a maximum is present."
    let has_maximum = reader.load_imm_u8()?;
    let initial = reader.load_imm_varuint32()?;
    let maximum = if has_maximum == 1 {
        Some(reader.load_imm_varuint32()?)
    } else {
        None
    };

    let limits = (initial, maximum);
    // If the limits is malformed (too large, etc.), that's a problem.
    if limits.0 > MAX_MEMORY_SIZE_PAGES {
        return Err(MalformedMemory("Memory limits are too large".to_string()));
    }
    if let Some(max) = limits.1 {
        if max > MAX_MEMORY_SIZE_PAGES {
            return Err(MalformedMemory("Memory limits are too large".to_string()));
        }
        if max < limits.0 {
            return Err(MalformedMemory(
                "Maximum memory size is less than minimum".to_string(),
            ));
        }
    }

    Ok(limits)
}

fn read_table(reader: &mut LEB128Reader) -> Result<Table, LoaderError> {
    let ty = reader.load_imm_u8().map_err(DecoderError)?;
    let ty = ReferenceType::from_u8(ty)?;

    let limits = read_limits(reader).map_err(DecoderError)?;
    Ok(Table { ty, limits })
}

const MAX_MEMORY_SIZE_PAGES: u32 = 0x10000;

impl Module {
    pub fn load(module_data: &[u8]) -> Result<Self, LoaderError> {
        // Check for the WASM magic number
        if module_data.len() < 4 || &module_data[0..4] != b"\0asm" {
            return Err(LoaderError::InvalidMagicNumber);
        }

        if module_data.len() < 8 {
            return Err(LoaderError::InvalidVersion);
        }
        // Check for the WASM version
        let version = u32::from_le_bytes(
            module_data[4..8]
                .try_into()
                .map_err(|_| LoaderError::InvalidVersion)?,
        );
        if version != 1 {
            return Err(LoaderError::InvalidVersion);
        }

        // Now start parsing sections, we'll use Memory to read the bytes, as it has the necessary
        // functions to read LEB128 encoded integers and so on.
        let mut reader = LEB128Reader::new(module_data, 8);
        let mut tables = vec![];
        let mut exports = vec![];
        let mut imports = vec![];
        let mut types = vec![];
        let mut functions = vec![];
        let mut code = vec![];
        let mut memories = vec![];
        let mut globals = vec![];
        let mut data = vec![];
        let mut element_segments = vec![];
        let mut start_function = None;
        let mut data_count = None;
        while reader.remaining() > 0 {
            // Read the section ID
            let section_type = reader.load_imm_u8().map_err(DecoderError)?;

            // Read the section length
            let section_length = reader.load_imm_varuint32().map_err(DecoderError)?;
            let offset = reader.position();

            let section_type = SectionType::from_u8(section_type)?;

            match section_type {
                SectionType::Type => {
                    // Type section
                    let func_types = reader.load_imm_varuint32().map_err(DecoderError)?;

                    for _ in 0..func_types {
                        let func_type_marker = reader.load_imm_u8().map_err(DecoderError)?;
                        if func_type_marker != 0x60 {
                            return Err(DecoderError(FailedToDecode(format!(
                                "Invalid function type marker: expected 0x60, got 0x{func_type_marker:02x}"
                            ))));
                        }

                        let num_param_types = reader.load_imm_varuint32().map_err(DecoderError)?;
                        let mut params = Vec::with_capacity(num_param_types as usize);
                        for _ in 0..num_param_types {
                            let param_type = ValueType::read(&mut reader).map_err(DecoderError)?;
                            params.push(param_type);
                        }

                        let num_result_types = reader.load_imm_varuint32().map_err(DecoderError)?;
                        let mut results = Vec::with_capacity(num_result_types as usize);
                        for _ in 0..num_result_types {
                            let result_type = ValueType::read(&mut reader).map_err(DecoderError)?;
                            results.push(result_type);
                        }

                        types.push(FuncType { params, results });
                    }
                }
                SectionType::Function => {
                    // Function section, a vector of types
                    let num_functions = reader.load_imm_varuint32().map_err(DecoderError)?;

                    for _ in 0..num_functions {
                        let type_index = reader.load_imm_varuint32().map_err(DecoderError)?;
                        functions.push(type_index as usize);
                    }
                }
                SectionType::Export => {
                    // Export section
                    let num_exports = reader.load_imm_varuint32().map_err(DecoderError)?;

                    for _ in 0..num_exports {
                        let name = reader.load_string().map_err(DecoderError)?;
                        let kind = reader.load_imm_u8().map_err(DecoderError)?;
                        let kind = ImportExportKind::from_u8(kind)?;
                        let index = reader.load_imm_varuint32().map_err(DecoderError)?;

                        exports.push(ExportEntry { name, kind, index });
                    }
                }
                SectionType::Code => {
                    // Code section
                    let num_functions = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_functions {
                        let mut code_size =
                            reader.load_imm_varuint32().map_err(DecoderError)? as usize;
                        // Code size includes the locals block, so we chop that off after reading them.
                        let before_locals = reader.position();
                        let num_types = reader.load_imm_varuint32().map_err(DecoderError)?;
                        let mut locals = Vec::with_capacity(num_types as usize);
                        for _ in 0..num_types {
                            let count = reader.load_imm_varuint32().map_err(DecoderError)?;
                            // This is an obscene number of locals, so we'll just fail here.
                            // In all likelihood this is a malformed module. This number was chosen
                            // because of the binary WAST tests, but it probably could be much
                            // lower.
                            if count >= 0x40000000 {
                                return Err(DecoderError(FailedToDecode(
                                    "Too many locals in a function".to_string(),
                                )));
                            }
                            let ty = ValueType::read(&mut reader).map_err(DecoderError)?;
                            for _ in 0..count {
                                locals.push(ty);
                            }
                        }
                        code_size -= reader.position() - before_locals;
                        let func_offsets = (reader.position(), reader.position() + code_size);
                        code.push(Code {
                            locals,
                            code: func_offsets,
                        });
                        reader.advance(code_size);
                    }
                }
                SectionType::Import => {
                    // Import section
                    let num_imports = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_imports {
                        let module = reader.load_string().map_err(DecoderError)?;
                        let field = reader.load_string().map_err(DecoderError)?;
                        let kind = reader.load_imm_u8().map_err(DecoderError)?;

                        let kind = ImportExportKind::from_u8(kind)?;
                        let import = match kind {
                            ImportExportKind::Function => {
                                let function_index =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                Import::Func(function_index)
                            }
                            ImportExportKind::Table => {
                                let reftype = reader.load_imm_u8().map_err(DecoderError)?;
                                let reftype = ReferenceType::from_u8(reftype)?;
                                let limits = read_limits(&mut reader).map_err(DecoderError)?;
                                Import::Table(reftype, limits)
                            }
                            ImportExportKind::Memory => {
                                let limits = read_limits(&mut reader).map_err(DecoderError)?;

                                Import::Memory(limits)
                            }
                            ImportExportKind::Global => {
                                let valtype = ValueType::read(&mut reader).map_err(DecoderError)?;
                                let is_mut = reader.load_imm_u8().map_err(DecoderError)? == 1;
                                Import::Global(valtype, is_mut)
                            }
                        };
                        imports.push((module, field, import));
                    }
                }
                SectionType::Table => {
                    // Table section
                    let num_tables = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_tables {
                        let t = read_table(&mut reader)?;

                        tables.push(t);
                    }
                }
                SectionType::Element => {
                    // A vector of element segments.
                    let num_segments = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_segments {
                        let flags = reader.load_imm_varuint32().map_err(DecoderError)?;
                        assert!(flags <= 7);

                        let es = match flags {
                            0 => {
                                let init_expr = reader.load_expr().map_err(DecoderError)?;
                                let num_func_indices =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let func_indices = (0..num_func_indices)
                                    .map(|_| reader.load_imm_varuint32().map_err(DecoderError))
                                    .collect::<Result<Vec<u32>, _>>()?;
                                ElementSegment {
                                    reftype: ReferenceType::FuncRef,
                                    elements: Elements::Function(func_indices),
                                    mode: ElementMode::Active {
                                        table_index: 0,
                                        expr: init_expr,
                                    },
                                }
                            }
                            1 => {
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                let reftype = if kind == 0 {
                                    ReferenceType::FuncRef
                                } else {
                                    ReferenceType::from_u8(kind)?
                                };
                                let num_func_indices =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let func_indices = (0..num_func_indices)
                                    .map(|_| reader.load_imm_varuint32().map_err(DecoderError))
                                    .collect::<Result<Vec<u32>, _>>()?;
                                ElementSegment {
                                    reftype,
                                    elements: Elements::Function(func_indices),
                                    mode: ElementMode::Passive,
                                }
                            }
                            2 => {
                                let table_index =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let init_expr = reader.load_expr().map_err(DecoderError)?;
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                if kind != 0 {
                                    return Err(DecoderError(FailedToDecode(format!(
                                        "Unsupported element kind: {kind}"
                                    ))));
                                }
                                let num_func_indices =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let func_indices = (0..num_func_indices)
                                    .map(|_| reader.load_imm_varuint32().map_err(DecoderError))
                                    .collect::<Result<Vec<u32>, _>>()?;
                                ElementSegment {
                                    reftype: ReferenceType::FuncRef,
                                    elements: Elements::Function(func_indices),
                                    mode: ElementMode::Active {
                                        table_index,
                                        expr: init_expr,
                                    },
                                }
                            }
                            3 => {
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                if kind != 0 {
                                    return Err(DecoderError(FailedToDecode(format!(
                                        "Unsupported element kind: {kind}"
                                    ))));
                                }
                                let num_func_indices =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let func_indices = (0..num_func_indices)
                                    .map(|_| reader.load_imm_varuint32().map_err(DecoderError))
                                    .collect::<Result<Vec<u32>, _>>()?;
                                ElementSegment {
                                    reftype: ReferenceType::FuncRef,
                                    elements: Elements::Function(func_indices),
                                    mode: ElementMode::Declarative,
                                }
                            }
                            4 => {
                                let init_expr = reader.load_expr().map_err(DecoderError)?;
                                let num_elem_exprs =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let elem_exprs = (0..num_elem_exprs)
                                    .map(|_| reader.load_expr().map_err(DecoderError))
                                    .collect::<Result<Vec<Region>, _>>()?;
                                ElementSegment {
                                    reftype: ReferenceType::FuncRef,
                                    elements: Elements::Expression(elem_exprs),
                                    mode: ElementMode::Active {
                                        table_index: 0,
                                        expr: init_expr,
                                    },
                                }
                            }
                            5 => {
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                let reftype = if kind == 0 {
                                    ReferenceType::FuncRef
                                } else {
                                    ReferenceType::from_u8(kind)?
                                };
                                let num_elem_exprs =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let elem_exprs = (0..num_elem_exprs)
                                    .map(|_| reader.load_expr().map_err(DecoderError))
                                    .collect::<Result<Vec<Region>, _>>()?;
                                ElementSegment {
                                    reftype,
                                    elements: Elements::Expression(elem_exprs),
                                    mode: ElementMode::Passive,
                                }
                            }
                            6 => {
                                let table_index =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let init_expr = reader.load_expr().map_err(DecoderError)?;
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                let reftype = if kind == 0 {
                                    ReferenceType::FuncRef
                                } else {
                                    ReferenceType::from_u8(kind)?
                                };
                                let num_elem_exprs =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let elem_exprs = (0..num_elem_exprs)
                                    .map(|_| reader.load_expr().map_err(DecoderError))
                                    .collect::<Result<Vec<Region>, _>>()?;
                                ElementSegment {
                                    reftype,
                                    elements: Elements::Expression(elem_exprs),
                                    mode: ElementMode::Active {
                                        table_index,
                                        expr: init_expr,
                                    },
                                }
                            }
                            7 => {
                                let kind = reader.load_imm_u8().map_err(DecoderError)?;
                                let reftype = if kind == 0 {
                                    ReferenceType::FuncRef
                                } else {
                                    ReferenceType::from_u8(kind)?
                                };
                                let num_elem_exprs =
                                    reader.load_imm_varuint32().map_err(DecoderError)?;
                                let elem_exprs = (0..num_elem_exprs)
                                    .map(|_| reader.load_expr().map_err(DecoderError))
                                    .collect::<Result<Vec<Region>, _>>()?;
                                ElementSegment {
                                    reftype,
                                    elements: Elements::Expression(elem_exprs),
                                    mode: ElementMode::Declarative,
                                }
                            }
                            _ => {
                                return Err(DecoderError(FailedToDecode(format!(
                                    "Unsupported element segment flags: {flags}"
                                ))));
                            }
                        };
                        element_segments.push(es);
                    }
                }
                SectionType::Memory => {
                    // Memory section
                    let num_memories = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_memories {
                        let limits = read_limits(&mut reader).map_err(DecoderError)?;
                        memories.push(MemorySection { limits });
                    }
                }
                SectionType::Global => {
                    // Global section
                    let num_globals = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_globals {
                        let ty = ValueType::read(&mut reader).map_err(DecoderError)?;
                        let mut_flag = reader.load_imm_u8().map_err(DecoderError)?;
                        let mutable = mut_flag == 1;
                        let expr = reader.load_expr().map_err(DecoderError)?;
                        globals.push(Global { ty, mutable, expr });
                    }
                }
                SectionType::Data => {
                    let num_data = reader.load_imm_varuint32().map_err(DecoderError)?;
                    for _ in 0..num_data {
                        let memtype = reader.load_imm_varuint32().map_err(DecoderError)?;
                        let datum = match memtype {
                            0 => {
                                // Active
                                let expr = reader.load_expr().map_err(DecoderError)?;
                                // Read the data, which is a vector of bytes.
                                // The first byte in the vector is 'initial', which in the current
                                // module format is always 0.
                                let data = reader.load_data().map_err(DecoderError)?;
                                Data::Active { expr, data }
                            }
                            1 => {
                                // Passive
                                let data = reader.load_data().map_err(DecoderError)?;
                                Data::Passive { data }
                            }
                            2 => {
                                // ActiveMemIdx
                                let memidx = reader.load_imm_varuint32().map_err(DecoderError)?;
                                let expr = reader.load_expr().map_err(DecoderError)?;
                                let data = reader.load_data().map_err(DecoderError)?;
                                Data::ActiveMemIdx { memidx, expr, data }
                            }
                            _ => {
                                return Err(DecoderError(InvalidDataSegmentType(memtype)));
                            }
                        };
                        data.push(datum);
                    }
                }
                SectionType::Start => {
                    // "The start section has the id 8. It decodes into an optional start function that represents the
                    //  component of a module."
                    let funcidx = reader.load_imm_varuint32().map_err(DecoderError)?;
                    start_function = Some(funcidx as usize);
                }
                SectionType::Custom => {
                    // Parse custom section to validate internal LEB128 encoding
                    let section_end = reader.position() + section_length as usize;

                    // Read and validate the custom section name
                    let _name = reader.load_string().map_err(DecoderError)?;

                    // Skip the rest of the custom section content
                    let remaining = section_end - reader.position();
                    reader.advance(remaining);
                }
                SectionType::DataCount => {
                    data_count = Some(reader.load_imm_varuint32().map_err(DecoderError)?);
                }
            }

            let what_we_read = reader.position() - offset;
            if what_we_read != section_length as usize {
                return Err(DecoderError(FailedToDecode(
                    format!(
                        "Section length mismatch. We have {} bytes left, we should have read {} bytes, but we read {} bytes. Section type was {:?}",
                        reader.remaining(),
                        section_length,
                        what_we_read,
                        section_type
                    ),
                )));
            }
        }

        // Nothing should be remaining in the reader.
        if reader.remaining() > 0 {
            return Err(DecoderError(FailedToDecode(format!(
                "Reader has {} bytes remaining",
                reader.remaining()
            ))));
        }

        // # of code must equal functions or this is malformed.
        if code.iter().len() != functions.len() {
            return Err(DecoderError(FailedToDecode(
                "Code section length does not match function section length".to_string(),
            )));
        }

        // Data count must be equal to the number of data segments, if it's been specified
        if let Some(data_count) = data_count {
            if data_count != data.len() as u32 {
                return Err(DecoderError(FailedToDecode(
                    "Data count does not match the number of data segments".to_string(),
                )));
            }
        }

        Ok(Module {
            module_data: module_data.to_vec(),
            version,
            tables,
            exports,
            imports,
            types,
            functions,
            code,
            memories,
            globals,
            data,
            start_function,
            element_segments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Module;

    #[test]
    fn verify_section_loading_table() {
        let mod_data = include_bytes!("../../tests/table.wasm").to_vec();
        let program = Module::load(&mod_data).unwrap();

        // Verify basic counts of things
        assert_eq!(program.version, 1);
        assert_eq!(program.tables.len(), 1);
        assert_eq!(program.exports.len(), 2);
        assert_eq!(program.imports.len(), 1);
        assert_eq!(program.types.len(), 1);
        assert_eq!(program.functions.len(), 3);
        assert_eq!(program.code.len(), program.functions.len());

        // Verify the tables
        assert_eq!(
            program.tables,
            vec![Table {
                ty: ReferenceType::FuncRef,
                limits: (32, None),
            }]
        );

        // Verify the exports
        assert_eq!(
            program.exports,
            vec![
                ExportEntry {
                    name: "times2".to_string(),
                    kind: ImportExportKind::Function,
                    index: 2,
                },
                ExportEntry {
                    name: "times3".to_string(),
                    kind: ImportExportKind::Function,
                    index: 3,
                },
            ]
        );

        // Verify the types
        assert_eq!(
            program.types,
            vec![FuncType {
                params: vec![ValueType::I32],
                results: vec![ValueType::I32],
            }]
        );

        // Verify the functions
        assert_eq!(
            program.functions,
            vec![0, 0, 0] // 0 is the index into the types array
        );

        // Verify code offsets
        assert_eq!(program.code[0].locals, vec![]);
        assert_eq!(program.code[1].locals, vec![]);
        assert_eq!(program.code[2].locals, vec![]);
    }

    #[test]
    fn test_verify_section_loading_itoa() {
        let mod_data = include_bytes!("../../tests/itoa.wasm").to_vec();
        let program = Module::load(&mod_data).unwrap();

        assert_eq!(program.version, 1);
        assert_eq!(program.tables.len(), 0);
        assert_eq!(program.exports.len(), 2);
        assert_eq!(program.imports.len(), 1);
        assert_eq!(program.types.len(), 2);
        assert_eq!(program.functions.len(), 1);
        assert_eq!(program.code.len(), program.functions.len());
        assert_eq!(program.memories.len(), 1);
        assert_eq!(program.globals.len(), 1);
        assert_eq!(program.data.len(), 1);

        // Verify the exports
        assert_eq!(
            program.exports,
            vec![
                ExportEntry {
                    name: "memory".to_string(),
                    kind: ImportExportKind::Memory,
                    index: 0,
                },
                ExportEntry {
                    name: "itoa".to_string(),
                    kind: ImportExportKind::Function,
                    index: 1,
                },
            ]
        );

        // Verify the types
        assert_eq!(
            program.types,
            vec![
                FuncType {
                    params: vec![ValueType::I32],
                    results: vec![],
                },
                FuncType {
                    params: vec![ValueType::I32],
                    results: vec![ValueType::I32, ValueType::I32],
                }
            ]
        );

        // Verify the functions
        assert_eq!(program.functions, vec![1]);

        // Verify the memories
        assert_eq!(program.memories, vec![MemorySection { limits: (1, None) }]);

        // Verify the globals
        assert_eq!(
            program.globals,
            vec![Global {
                ty: ValueType::I32,
                mutable: false,
                expr: (48, 51)
            }]
        );

        // Verify the data
        assert_eq!(
            program.data,
            vec![Data::Active {
                expr: (196, 199),
                data: (201, 211),
            }]
        );
    }
}
