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

use crate::decode::{decode, Program, ScopeType};
use crate::exec::{eval_init_expr, Fault, GlobalVar, Value};
use crate::frame::Frame;
use crate::import::{ImportedFunction, ImportResolver};
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
    pub imported_functions: Vec<ImportedFunction>,
}

/// Produce an instance from a module with import resolution.
pub fn mk_instance_with_imports(
    module: Module,
    resolver: &dyn ImportResolver,
) -> Result<Instance, LinkError> {
    // Resolve imports first
    let mut imported_functions = Vec::new();
    let mut imported_globals = Vec::new();
    let mut imported_tables = Vec::new();
    let mut imported_memories = Vec::new();
    
    for (module_name, field_name, import) in &module.imports {
        match import {
            crate::module::Import::Func(_type_idx) => {
                if let Some(imported_func) = resolver.resolve_function(module_name, field_name) {
                    imported_functions.push(imported_func);
                } else {
                    return Err(LinkError::UnsupportedFeature(format!(
                        "Cannot resolve import: {module_name}::{field_name}"
                    )));
                }
            }
            crate::module::Import::Global(_value_type, _mutable) => {
                if let Some(imported_global) = resolver.resolve_global(module_name, field_name) {
                    imported_globals.push(imported_global);
                } else {
                    return Err(LinkError::UnsupportedFeature(format!(
                        "Cannot resolve import: {module_name}::{field_name}"
                    )));
                }
            }
            crate::module::Import::Table(_ref_type, _limits) => {
                if let Some(imported_table) = resolver.resolve_table(module_name, field_name) {
                    imported_tables.push(imported_table);
                } else {
                    return Err(LinkError::UnsupportedFeature(format!(
                        "Cannot resolve import: {module_name}::{field_name}"
                    )));
                }
            }
            crate::module::Import::Memory(_limits) => {
                if let Some(imported_memory) = resolver.resolve_memory(module_name, field_name) {
                    imported_memories.push(imported_memory);
                } else {
                    return Err(LinkError::UnsupportedFeature(format!(
                        "Cannot resolve import: {module_name}::{field_name}"
                    )));
                }
            }
        }
    }

    // Create instance with the resolved imports
    mk_instance_internal(module, imported_functions, imported_globals, imported_tables, imported_memories)
}

/// Produce an instance from a module.
pub fn mk_instance(module: Module) -> Result<Instance, LinkError> {
    // For modules without imports, use the original logic to avoid any regressions
    mk_instance_internal(module, Vec::new(), Vec::new(), Vec::new(), Vec::new())
}

fn mk_instance_internal(
    module: Module,
    imported_functions: Vec<ImportedFunction>,
    imported_globals: Vec<Value>,
    imported_tables: Vec<TableInstance>,
    imported_memories: Vec<VectorMemory>,
) -> Result<Instance, LinkError> {
    // Check if this module has any imports
    let has_imports = !imported_functions.is_empty() || !imported_globals.is_empty() || !imported_tables.is_empty() || !imported_memories.is_empty();
    
    // Create imported globals context first for initialization expressions
    let mut imported_global_vars = Vec::new();
    for imported_global in &imported_globals {
        let dummy_decl = crate::module::Global {
            ty: imported_global.type_of(),
            mutable: false, // TODO: Get actual mutability from import declaration
            expr: (0, 0), // Empty expression range
        };
        imported_global_vars.push(GlobalVar {
            decl: dummy_decl,
            value: *imported_global,
        });
    }
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

    // Initialize memories: imported memories first, then local memories
    let mut memories: Vec<_> = imported_memories.into_iter().collect();
    memories.extend(module
        .memories
        .iter()
        .map(|m_decl| {
            let min_pages = m_decl.limits.0;
            let max_pages = m_decl.limits.1;
            VectorMemory::new(
                min_pages as usize * WASM_PAGE_SIZE,
                max_pages.map(|x| x as usize * WASM_PAGE_SIZE),
            )
        }));

    // Initialize tables: imported tables first, then local tables
    let mut tables: Vec<_> = imported_tables.into_iter().collect();
    tables.extend(module
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
        }));

    // Apply active element segments to initialize tables
    for element_segment in &module.element_segments {
        if let crate::module::ElementMode::Active { table_index, expr } = &element_segment.mode {
            let table_idx = *table_index as usize;
            if table_idx < tables.len() {
                if let crate::module::Elements::Function(func_indices) = &element_segment.elements {
                    // Evaluate the init expression to get the offset
                    let offset_value = if has_imports {
                        // Build complete function index space: [imported_functions] + [local_functions]
                        let mut function_types = Vec::new();
                        for (_, _, import) in &module.imports {
                            if let crate::module::Import::Func(type_idx) = import {
                                function_types.push(*type_idx as usize);
                            }
                        }
                        function_types.extend_from_slice(&module.functions);
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &imported_global_vars, &module.types, &function_types)
                    } else {
                        // Even without imports, we need to pass function information for ref.func to work
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &[], &module.types, &module.functions)
                    }.map_err(LinkError::ActiveExpressionError)?;
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
                    let data_offset = if has_imports {
                        // Build complete function index space: [imported_functions] + [local_functions]
                        let mut function_types = Vec::new();
                        for (_, _, import) in &module.imports {
                            if let crate::module::Import::Func(type_idx) = import {
                                function_types.push(*type_idx as usize);
                            }
                        }
                        function_types.extend_from_slice(&module.functions);
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &imported_global_vars, &module.types, &function_types)
                    } else {
                        // Even without imports, we need to pass function information for ref.func to work
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &[], &module.types, &module.functions)
                    }.map_err(LinkError::ActiveExpressionError)?;
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
                    let data_offset = if has_imports {
                        // Build complete function index space: [imported_functions] + [local_functions]
                        let mut function_types = Vec::new();
                        for (_, _, import) in &module.imports {
                            if let crate::module::Import::Func(type_idx) = import {
                                function_types.push(*type_idx as usize);
                            }
                        }
                        function_types.extend_from_slice(&module.functions);
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &imported_global_vars, &module.types, &function_types)
                    } else {
                        // Even without imports, we need to pass function information for ref.func to work
                        eval_init_expr(module.get_expr(expr), ValueType::I32, &[], &module.types, &module.functions)
                    }.map_err(LinkError::ActiveExpressionError)?;
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

    // Populate globals: imported globals first, then local globals
    let mut globals = Vec::with_capacity(imported_globals.len() + module.globals.len());
    
    // Add imported globals first (reuse the ones we created for initialization context)
    globals.extend(imported_global_vars.iter().cloned());
    
    // Then add local globals
    for global_segment in &module.globals {
        // Execute the expression in the global
        let program = module.get_expr(&global_segment.expr);
        let result = if has_imports {
            // Build complete function index space: [imported_functions] + [local_functions]
            let mut function_types = Vec::new();
            for (_, _, import) in &module.imports {
                if let crate::module::Import::Func(type_idx) = import {
                    function_types.push(*type_idx as usize);
                }
            }
            function_types.extend_from_slice(&module.functions);
            eval_init_expr(program, global_segment.ty, &imported_global_vars, &module.types, &function_types)
        } else {
            // Even without imports, we need to pass function information for ref.func to work
            eval_init_expr(program, global_segment.ty, &[], &module.types, &module.functions)
        }.map_err(LinkError::ActiveExpressionError)?;
        globals.push(GlobalVar {
            decl: global_segment.clone(),
            value: result,
        });
    }

    let instance = Instance {
        module,
        memories,
        globals,
        programs,
        tables,
        imported_functions,
    };

    // Execute start function if present
    if let Some(start_func_idx) = instance.module.start_function {
        // Create execution context and run the start function
        use crate::{ExecutionState, VectorMemory};

        let memory = if !instance.memories.is_empty() {
            instance.memories[0].clone()
        } else {
            VectorMemory::new(0, None)
        };

        let mut execution = ExecutionState::new(memory);
        execution.add_module(0, instance);
        execution
            .prepare(0, start_func_idx as u32, &[])
            .map_err(|e| match e {
                crate::exec::ExecError::ExecutionFault(f) => LinkError::ActiveExpressionError(f),
                crate::exec::ExecError::LinkageError(l) => l,
            })?;
        execution.run().map_err(|e| match e {
            crate::exec::ExecError::ExecutionFault(f) => LinkError::ActiveExpressionError(f),
            crate::exec::ExecError::LinkageError(l) => l,
        })?;

        // Extract the instance back from execution (automatically syncs memory)
        let updated_instance = execution.take_module(0).unwrap();
        Ok(updated_instance)
    } else {
        Ok(instance)
    }
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
        // Function index space: [imported_functions] + [local_functions]
        // Count imported functions from the module's import section
        let mut num_imported_funcs = 0;
        for (_, _, import) in self.module.imports.iter() {
            match import {
                crate::module::Import::Func(_type_idx) => {
                    num_imported_funcs += 1;
                }
                _ => continue,
            }
        }
        
        // If index is within imported functions range, we can't create a frame for it
        // (imported functions are dispatched via continuation mechanism)
        if index < num_imported_funcs {
            return Err(LinkError::UnsupportedFeature(
                "Cannot create frame for imported function - use continuation dispatch".to_string(),
            ));
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
