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

use crate::decode::{decode, Program, ScopeType};
use crate::exec::{exec_fragment, Fault, GlobalVar, Value};
use crate::frame::Frame;
use crate::module::{Data, ReferenceType};
use crate::stack::Stack;
use crate::{DecodeError, Module, Type, ValueType, VectorMemory};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const WASM_PAGE_SIZE: usize = 1 << 16;

/// Runtime representation of a table
#[derive(Debug, Clone)]
pub struct TableInstance {
    pub elements: Vec<Option<Value>>,
    pub ref_type: ReferenceType,
    pub limits: (u32, Option<u32>),
}

#[derive(Debug)]
pub enum LinkError {
    ActiveExpressionError(Fault),
    DecodeError(DecodeError),
    FunctionNotFound,
    UnsupportedFeature(String),
    ArgumentTypeMismatch(usize, ValueType, ValueType),
    MissingMemory,
}

impl Display for LinkError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::ActiveExpressionError(e) => write!(f, "Active expression error: {e}"),
            LinkError::FunctionNotFound => write!(f, "Function not found"),
            LinkError::UnsupportedFeature(s) => write!(f, "Unsupported feature: {s}"),
            LinkError::ArgumentTypeMismatch(idx, expected, actual) => write!(
                f,
                "Argument type mismatch at index {idx}: expected {expected:?}, got {actual:?}"
            ),
            LinkError::MissingMemory => write!(f, "No memory found"),
            LinkError::DecodeError(e) => write!(f, "Decode error: {e}"),
        }
    }
}

impl Error for LinkError {}

pub struct Instance {
    pub module: Module,
    pub memories: Vec<VectorMemory>,
    pub globals: Vec<GlobalVar>,
    pub programs: Vec<Program>,
    pub tables: Vec<TableInstance>,
}

/// Produce an instance from a module.
pub fn mk_instance(module: Module) -> Result<Instance, LinkError> {
    let mut programs = Vec::with_capacity(module.code.len());

    for (i, code) in module.code.iter().enumerate() {
        let program_memory = module.code(i);
        let mut program = decode(program_memory).map_err(LinkError::DecodeError)?;

        // Make local types from function signatures + code local signatures
        let typeidx = module.functions[i];
        let num_locals = code.locals.len() + module.types[typeidx].params.len();
        let mut local_types = Vec::with_capacity(num_locals);
        for param_type in &module.types[typeidx].params {
            local_types.push(*param_type);
        }
        for local_type in &module.code[i].locals {
            local_types.push(*local_type);
        }

        program.local_types = local_types;
        program.return_types = module.types[typeidx].results.clone();

        programs.push(program);
    }

    let mut memories: Vec<_> = module
        .memories
        .iter()
        .map(|m_decl| {
            let min_pages = m_decl.limits.0;
            let max_pages = m_decl.limits.1;
            VectorMemory::new(
                min_pages as usize * WASM_PAGE_SIZE,
                max_pages.map(|x| x as usize * WASM_PAGE_SIZE),
            )
        })
        .collect();

    // Initialize tables
    let mut tables: Vec<_> = module
        .tables
        .iter()
        .map(|t_decl| {
            let min_size = t_decl.limits.0;
            let max_size = t_decl.limits.1;
            TableInstance {
                elements: vec![None; min_size as usize],
                ref_type: t_decl.ty,
                limits: (min_size, max_size),
            }
        })
        .collect();

    // Apply active element segments to initialize tables
    for element_segment in &module.element_segments {
        if let crate::module::ElementMode::Active {
            table_index,
            expr,
        } = &element_segment.mode
        {
            let table_idx = *table_index as usize;
            if table_idx < tables.len() {
                if let crate::module::Elements::Function(func_indices) = &element_segment.elements {
                    // Evaluate the init expression to get the offset
                    let offset_value = exec_fragment(module.get_expr(expr), ValueType::I32)
                        .map_err(LinkError::ActiveExpressionError)?;
                    let Value::I32(offset) = offset_value else {
                        panic!("Element segment offset must be i32");
                    };
                    let offset = offset as usize;
                    for (i, &func_idx) in func_indices.iter().enumerate() {
                        if offset + i < tables[table_idx].elements.len() {
                            tables[table_idx].elements[offset + i] =
                                Some(Value::FuncRef(Some(func_idx)));
                        }
                    }
                }
            }
        }
    }

    // Support modules without memory
    if memories.len() > 1 {
        return Err(LinkError::UnsupportedFeature(
            "Multiple memories not supported yet".to_string(),
        ));
    }

    // Populate memory from global data (only if memory exists).
    if !memories.is_empty() {
        for data_segment in &module.data {
            match data_segment {
                Data::Active { expr, data } => {
                    // We have to execute the program located at expr in order to get the address
                    // of the data segment.
                    let data_offset = exec_fragment(module.get_expr(expr), ValueType::I32)
                        .map_err(LinkError::ActiveExpressionError)?;
                    let Value::I32(data_offset) = data_offset else {
                        panic!("Data segment offset must be i32");
                    };
                    let data_offset = data_offset as usize;
                    // Read from program memory @ data offset into memory_vec
                    let data_len = data.1 - data.0;
                    memories[0].data_mut()[data_offset..data_offset + data_len]
                        .copy_from_slice(&module.module_data[data.0..data.1]);
                }
                Data::ActiveMemIdx { memidx, expr, data } => {
                    // This is identical to above but with a memory index set. But standard doesn't
                    // support multiple memories yet. But we'll just go ahead and implement it.
                    let data_offset = exec_fragment(module.get_expr(expr), ValueType::I32)
                        .map_err(LinkError::ActiveExpressionError)?;
                    let Value::I32(data_offset) = data_offset else {
                        panic!("Data segment offset must be i32");
                    };
                    let data_offset = data_offset as usize;
                    memories[*memidx as usize].data_mut()
                        [data_offset..data_offset + data.1 - data.0]
                        .copy_from_slice(&module.module_data[data.0..data.1]);
                }
                Data::Passive { data } => {
                    let offset = data.0;
                    let end = data.1;
                    memories[0].data_mut()[offset..end]
                        .copy_from_slice(&module.module_data[offset..end]);
                }
            }
        }
    }

    // Populate globals.
    let mut globals = Vec::with_capacity(module.globals.len());
    for global_segment in &module.globals {
        // Execute the expression in the global
        let program = module.get_expr(&global_segment.expr);
        let result =
            exec_fragment(program, global_segment.ty).map_err(LinkError::ActiveExpressionError)?;
        globals.push(GlobalVar {
            decl: global_segment.clone(),
            value: result,
        });
    }

    Ok(Instance {
        module,
        memories,
        globals,
        programs,
        tables,
    })
}

impl Instance {
    pub fn find_funcidx(&self, name: &str) -> Option<u32> {
        for export in &self.module.exports {
            if export.name == name {
                match export.kind {
                    crate::module::ImportExportKind::Function => {
                        return Some(export.index);
                    }
                    _ => continue,
                }
            }
        }
        None
    }

    pub fn frame_for_funcidx(&self, index: u32, args: &[Value]) -> Result<Frame, LinkError> {
        // Funcidx must consider also the imports, it isn't just an offset into `code` section.
        // So to find the function index, scan imports first
        // Then scan functions/code.
        // We don't actually handle imports yet, so that's a panic if it's in that space.
        // We could make this more efficient by precomputing the number of imported functions, and
        // stashing that in the linked struct, or even having a map of funcidx to code idx.
        let mut num_imported_funcs = 0;
        for (_, _, import) in self.module.imports.iter() {
            match import {
                crate::module::Import::Func(idx) => {
                    num_imported_funcs += 1;
                    if *idx == index {
                        return Err(LinkError::UnsupportedFeature(
                            "Imported functions not supported yet".to_string(),
                        ));
                    }
                }
                _ => continue,
            }
        }
        let funcidx = index - num_imported_funcs;
        let typeindx = self.module.functions[funcidx as usize];
        // Types of arguments must match the function signature
        for (i, (expected, actual)) in self.module.types[typeindx]
            .params
            .iter()
            .zip(args.iter())
            .enumerate()
        {
            // TODO: this doesn't seem to work with the itoa example?!
            if *expected != actual.type_of() {
                return Err(LinkError::ArgumentTypeMismatch(
                    i,
                    *expected,
                    actual.type_of(),
                ));
            }
        }
        let index = (index - num_imported_funcs) as usize;
        if index >= self.programs.len() {
            return Err(LinkError::FunctionNotFound);
        }
        let program = &self.programs[index];
        let num_locals = program.local_types.len();
        let mut locals = args.to_vec();

        // Initialize remaining local variables to their zero values based on their types
        for i in args.len()..num_locals {
            let local_type = program.local_types[i];
            let zero_value = match local_type {
                ValueType::I32 => Value::I32(0),
                ValueType::I64 => Value::I64(0),
                ValueType::F32 => Value::F32(0.0),
                ValueType::F64 => Value::F64(0.0),
                ValueType::Unit => Value::Unit,
                ValueType::V128 => Value::V128(0),
                ValueType::FuncRef => Value::FuncRef(None),
                ValueType::ExternRef => Value::ExternRef(None),
            };
            locals.push(zero_value);
        }

        let return_types = program.return_types.clone();
        let mut frame = Frame {
            locals,
            return_types,
            program: program.clone(),
            stack: Stack::new(),
            pc: 0,
            control_stack: vec![],
        };

        // Add function scope to control stack for proper branching
        // The function signature for branching purposes uses the function's return type
        let func_signature = if frame.return_types.is_empty() {
            Type::ValueType(ValueType::Unit)
        } else if frame.return_types.len() == 1 {
            Type::ValueType(frame.return_types[0])
        } else {
            // For multiple return values, we'd need a function type, but for now assume single return
            Type::ValueType(frame.return_types[0])
        };

        // Add function scope for proper control flow management
        frame.push_control(func_signature, ScopeType::Function);
        Ok(frame)
    }

    pub fn frame_for_funcname(&self, name: &str, args: &[Value]) -> Result<Frame, LinkError> {
        for export in &self.module.exports {
            if export.name == name {
                match export.kind {
                    crate::module::ImportExportKind::Function => {
                        return self.frame_for_funcidx(export.index, args);
                    }
                    _ => continue,
                }
            }
        }
        Err(LinkError::FunctionNotFound)
    }
}
