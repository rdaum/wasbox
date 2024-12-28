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

use crate::decode::{decode, Program};
use crate::exec::{exec_fragment, Fault, GlobalVar, Value};
use crate::frame::Frame;
use crate::module::Data;
use crate::stack::Stack;
use crate::{Module, ValueType, VectorMemory};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const WASM_PAGE_SIZE: usize = 1 << 16;

#[derive(Debug)]
pub enum LinkError {
    ActiveExpressionError(Fault),
    FunctionNotFound,
    UnsupportedFeature(String),
    ArgumentTypeMismatch(usize, ValueType, ValueType),
}

impl Display for LinkError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::ActiveExpressionError(e) => write!(f, "Active expression error: {}", e),
            LinkError::FunctionNotFound => write!(f, "Function not found"),
            LinkError::UnsupportedFeature(s) => write!(f, "Unsupported feature: {}", s),
            LinkError::ArgumentTypeMismatch(idx, expected, actual) => write!(
                f,
                "Argument type mismatch at index {}: expected {:?}, got {:?}",
                idx, expected, actual
            ),
        }
    }
}

impl Error for LinkError {}

pub struct Instance {
    pub module: Module,
    pub memories: Vec<VectorMemory>,
    pub globals: Vec<GlobalVar>,
    pub programs: Vec<Program>,
}

/// Produce an instance from a module.
pub fn mk_instance(module: Module) -> Result<Instance, LinkError> {
    let mut programs = Vec::with_capacity(module.code.len());

    // The funcidx in types etc here is relative to both imports and local functions, so we have to
    // have scanned imports to get the right index.
    let num_imported_funcs = module
        .imports
        .iter()
        .filter(|(_, _, import)| matches!(import, crate::module::Import::Func(_)))
        .count();

    for (i, code) in module.code.iter().enumerate() {
        let program_memory = module.code(i);
        let mut program = decode(program_memory).unwrap();

        // Make local types from function signatures + code local signatures

        let funcidx = i + num_imported_funcs;
        let num_locals = code.locals.len() + module.types[funcidx].params.len();
        let mut local_types = Vec::with_capacity(num_locals);
        for param_type in &module.types[funcidx].params {
            local_types.push(*param_type);
        }
        for local_type in &module.code[i].locals {
            local_types.push(*local_type);
        }

        program.local_types = local_types;
        program.return_types = module.types[funcidx].results.clone();

        programs.push(program);
    }

    let mut memories: Vec<_> = module
        .memories
        .iter()
        .map(|m_decl| {
            let min_pages = m_decl.limits.0;
            let max_pages = m_decl.limits.1;
            // We ignore the max_pages for now, we will need to get clever about using something
            // other than a vec, etc. to handle this.
            VectorMemory::new(
                min_pages as usize * WASM_PAGE_SIZE,
                max_pages.map(|x| x as usize * WASM_PAGE_SIZE),
            )
        })
        .collect();

    // Expectation is that there is only one memory for now.
    assert_eq!(memories.len(), 1);

    // Populate memory from global data.
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
                memories[*memidx as usize].data_mut()[data_offset..data_offset + data.1 - data.0]
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
    })
}

impl Instance {
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
        // Types of arguments must match the function signature
        for (i, (expected, actual)) in self.module.types[index as usize]
            .params
            .iter()
            .zip(args.iter())
            .enumerate()
        {
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
        locals.extend_from_slice(&vec![Value::Unit; num_locals - args.len()]);
        let return_types = program.return_types.clone();
        Ok(Frame {
            locals,
            return_types,
            program: program.clone(),
            stack: Stack::new(),
            pc: 0,
            control_stack: vec![],
        })
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
