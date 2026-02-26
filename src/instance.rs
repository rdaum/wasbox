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
use crate::exec::{eval_init_expr, register_detached_instance, Fault, FuncRef, GlobalVar, Value};
use crate::frame::Frame;
use crate::import::{ImportResolver, ImportedFunction};
use crate::memory::Memory;
use crate::module::{Data, ReferenceType};
use crate::stack::Stack;
use crate::{DecodeError, Module, Type, ValueType, VectorMemory};
use std::cell::RefCell;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

pub const WASM_PAGE_SIZE: usize = 1 << 16;
const FIRST_INSTANCE_OWNER_ID: u32 = 0x8000_0000;
static NEXT_INSTANCE_OWNER_ID: AtomicU32 = AtomicU32::new(FIRST_INSTANCE_OWNER_ID);

fn alloc_instance_owner_id() -> u32 {
    NEXT_INSTANCE_OWNER_ID.fetch_add(1, Ordering::Relaxed)
}

fn register_detached_snapshot(
    owner_id: u32,
    module: &Module,
    memories: &[VectorMemory],
    globals: &[GlobalVar],
    programs: &[Program],
    tables: &[TableInstance],
) {
    register_detached_instance(Instance {
        owner_id,
        module: module.clone(),
        memories: memories.to_vec(),
        globals: globals.to_vec(),
        programs: programs.to_vec(),
        tables: tables.to_vec(),
        imported_functions: vec![],
    });
}

/// Runtime representation of a table
#[derive(Debug, Clone)]
pub struct TableInstance {
    pub elements: Rc<RefCell<Vec<Option<Value>>>>,
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
    pub owner_id: u32,
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
    mk_instance_internal(
        module,
        imported_functions,
        imported_globals,
        imported_tables,
        imported_memories,
    )
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
    let owner_id = alloc_instance_owner_id();

    // Check if this module has any imports
    let has_imports = !imported_functions.is_empty()
        || !imported_globals.is_empty()
        || !imported_tables.is_empty()
        || !imported_memories.is_empty();

    // Create imported globals context first for initialization expressions
    let mut imported_global_vars = Vec::new();
    for imported_global in &imported_globals {
        let dummy_decl = crate::module::Global {
            ty: imported_global.type_of(),
            mutable: false, // TODO: Get actual mutability from import declaration
            expr: (0, 0),   // Empty expression range
        };
        imported_global_vars.push(GlobalVar {
            decl: dummy_decl,
            value: *imported_global,
        });
    }
    let mut programs = Vec::with_capacity(module.code.len());

    for (i, code) in module.code.iter().enumerate() {
        let program_memory = module.code(i);
        if program_memory.last().copied() != Some(0x0b) {
            return Err(LinkError::DecodeError(DecodeError::FailedToDecode(
                "END opcode expected".to_string(),
            )));
        }
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
    memories.extend(module.memories.iter().map(|m_decl| {
        let min_pages = m_decl.limits.0;
        let max_pages = m_decl.limits.1;
        VectorMemory::new(
            min_pages as usize * WASM_PAGE_SIZE,
            max_pages.map(|x| x as usize * WASM_PAGE_SIZE),
        )
    }));

    // Initialize tables: imported tables first, then local tables
    let mut tables: Vec<_> = imported_tables.into_iter().collect();
    tables.extend(module.tables.iter().map(|t_decl| {
        let min_size = t_decl.limits.0;
        let max_size = t_decl.limits.1;
        TableInstance {
            elements: Rc::new(RefCell::new(vec![None; min_size as usize])),
            ref_type: t_decl.ty,
            limits: (min_size, max_size),
        }
    }));

    // Apply active element segments to initialize tables
    for element_segment in &module.element_segments {
        if let crate::module::ElementMode::Active { table_index, expr } = &element_segment.mode {
            let table_idx = *table_index as usize;
            if table_idx < tables.len() {
                // Evaluate the init expression to get the table write offset.
                let offset_value = if has_imports {
                    let mut function_types = Vec::new();
                    for (_, _, import) in &module.imports {
                        if let crate::module::Import::Func(type_idx) = import {
                            function_types.push(*type_idx as usize);
                        }
                    }
                    function_types.extend_from_slice(&module.functions);
                    eval_init_expr(
                        module.get_expr(expr),
                        ValueType::I32,
                        &imported_global_vars,
                        &module.types,
                        &function_types,
                        owner_id,
                    )
                } else {
                    eval_init_expr(
                        module.get_expr(expr),
                        ValueType::I32,
                        &[],
                        &module.types,
                        &module.functions,
                        owner_id,
                    )
                }
                .map_err(LinkError::ActiveExpressionError)?;
                let Value::I32(offset) = offset_value else {
                    panic!("Element segment offset must be i32");
                };
                if offset < 0 {
                    register_detached_snapshot(
                        owner_id,
                        &module,
                        &memories,
                        &imported_global_vars,
                        &programs,
                        &tables,
                    );
                    return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                }
                let offset = offset as usize;

                match &element_segment.elements {
                    crate::module::Elements::Function(func_indices) => {
                        let mut table_elements = tables[table_idx].elements.borrow_mut();
                        let write_end = offset.checked_add(func_indices.len()).ok_or(
                            LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds),
                        )?;
                        if write_end > table_elements.len() {
                            drop(table_elements);
                            register_detached_snapshot(
                                owner_id,
                                &module,
                                &memories,
                                &imported_global_vars,
                                &programs,
                                &tables,
                            );
                            return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                        }
                        for (i, &func_idx) in func_indices.iter().enumerate() {
                            table_elements[offset + i] = Some(Value::FuncRef(Some(FuncRef {
                                module_id: owner_id,
                                func_idx,
                            })));
                        }
                    }
                    crate::module::Elements::Expression(exprs) => {
                        let mut table_elements = tables[table_idx].elements.borrow_mut();
                        let write_end = offset
                            .checked_add(exprs.len())
                            .ok_or(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds))?;
                        if write_end > table_elements.len() {
                            drop(table_elements);
                            register_detached_snapshot(
                                owner_id,
                                &module,
                                &memories,
                                &imported_global_vars,
                                &programs,
                                &tables,
                            );
                            return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                        }

                        for (i, expr) in exprs.iter().enumerate() {
                            let expected_type = match element_segment.reftype {
                                ReferenceType::FuncRef => ValueType::FuncRef,
                                ReferenceType::ExternRef => ValueType::ExternRef,
                            };
                            let value = if has_imports {
                                let mut function_types = Vec::new();
                                for (_, _, import) in &module.imports {
                                    if let crate::module::Import::Func(type_idx) = import {
                                        function_types.push(*type_idx as usize);
                                    }
                                }
                                function_types.extend_from_slice(&module.functions);
                                eval_init_expr(
                                    module.get_expr(expr),
                                    expected_type,
                                    &imported_global_vars,
                                    &module.types,
                                    &function_types,
                                    owner_id,
                                )
                            } else {
                                eval_init_expr(
                                    module.get_expr(expr),
                                    expected_type,
                                    &[],
                                    &module.types,
                                    &module.functions,
                                    owner_id,
                                )
                            }
                            .map_err(LinkError::ActiveExpressionError)?;
                            table_elements[offset + i] = Some(value);
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

    // Populate memory from global data.
    for data_segment in &module.data {
        match data_segment {
            Data::Active { .. } | Data::ActiveMemIdx { .. } if memories.is_empty() => {
                return Err(LinkError::MissingMemory);
            }
            _ => {}
        }
    }
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
                        eval_init_expr(
                            module.get_expr(expr),
                            ValueType::I32,
                            &imported_global_vars,
                            &module.types,
                            &function_types,
                            owner_id,
                        )
                    } else {
                        // Even without imports, we need to pass function information for ref.func to work
                        eval_init_expr(
                            module.get_expr(expr),
                            ValueType::I32,
                            &[],
                            &module.types,
                            &module.functions,
                            owner_id,
                        )
                    }
                    .map_err(LinkError::ActiveExpressionError)?;
                    let Value::I32(data_offset) = data_offset else {
                        panic!("Data segment offset must be i32");
                    };
                    let data_offset = data_offset as usize;
                    // Read from program memory @ data offset into memory_vec
                    let data_len = data.1 - data.0;
                    let Some(data_end) = data_offset.checked_add(data_len) else {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                    };
                    if data_end > memories[0].size() {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                    }
                    memories[0].data_mut()[data_offset..data_end]
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
                        eval_init_expr(
                            module.get_expr(expr),
                            ValueType::I32,
                            &imported_global_vars,
                            &module.types,
                            &function_types,
                            owner_id,
                        )
                    } else {
                        // Even without imports, we need to pass function information for ref.func to work
                        eval_init_expr(
                            module.get_expr(expr),
                            ValueType::I32,
                            &[],
                            &module.types,
                            &module.functions,
                            owner_id,
                        )
                    }
                    .map_err(LinkError::ActiveExpressionError)?;
                    let Value::I32(data_offset) = data_offset else {
                        panic!("Data segment offset must be i32");
                    };
                    let data_offset = data_offset as usize;
                    let data_len = data.1 - data.0;
                    let memory_idx = *memidx as usize;
                    if memory_idx >= memories.len() {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::MissingMemory);
                    }
                    let Some(data_end) = data_offset.checked_add(data_len) else {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                    };
                    if data_end > memories[memory_idx].size() {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                    }
                    memories[memory_idx].data_mut()[data_offset..data_end]
                        .copy_from_slice(&module.module_data[data.0..data.1]);
                }
                Data::Passive { data } => {
                    let offset = data.0;
                    let end = data.1;
                    if end > memories[0].size() {
                        register_detached_snapshot(
                            owner_id,
                            &module,
                            &memories,
                            &imported_global_vars,
                            &programs,
                            &tables,
                        );
                        return Err(LinkError::ActiveExpressionError(Fault::MemoryOutOfBounds));
                    }
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
            eval_init_expr(
                program,
                global_segment.ty,
                &imported_global_vars,
                &module.types,
                &function_types,
                owner_id,
            )
        } else {
            // Even without imports, we need to pass function information for ref.func to work
            eval_init_expr(
                program,
                global_segment.ty,
                &[],
                &module.types,
                &module.functions,
                owner_id,
            )
        }
        .map_err(LinkError::ActiveExpressionError)?;
        globals.push(GlobalVar {
            decl: global_segment.clone(),
            value: result,
        });
    }

    let instance = Instance {
        owner_id,
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
        use crate::ExecutionState;

        let mut execution = ExecutionState::new();
        execution.add_module(0, instance);
        execution
            .prepare(0, start_func_idx as u32, &[])
            .map_err(|e| match e {
                crate::exec::ExecError::ExecutionFault(f) => LinkError::ActiveExpressionError(f),
                crate::exec::ExecError::LinkageError(l) => l,
            })?;
        match execution.run() {
            Ok(()) => {
                // Extract the instance back from execution (automatically syncs memory)
                let updated_instance = execution.take_module(0).unwrap();
                Ok(updated_instance)
            }
            Err(e) => {
                // Preserve detached function bodies so imported tables can keep refs written
                // before the start trap.
                if let Some(detached_instance) = execution.take_module(0) {
                    register_detached_instance(detached_instance);
                }
                Err(match e {
                    crate::exec::ExecError::ExecutionFault(f) => LinkError::ActiveExpressionError(f),
                    crate::exec::ExecError::LinkageError(l) => l,
                })
            }
        }
    } else {
        Ok(instance)
    }
}

impl Instance {
    fn find_exportidx(&self, name: &str, kind: crate::module::ImportExportKind) -> Option<u32> {
        for export in &self.module.exports {
            if export.name == name && export.kind == kind {
                return Some(export.index);
            }
        }
        None
    }

    pub fn find_funcidx(&self, name: &str) -> Option<u32> {
        self.find_exportidx(name, crate::module::ImportExportKind::Function)
    }

    pub fn find_globalidx(&self, name: &str) -> Option<u32> {
        self.find_exportidx(name, crate::module::ImportExportKind::Global)
    }

    pub fn find_tableidx(&self, name: &str) -> Option<u32> {
        self.find_exportidx(name, crate::module::ImportExportKind::Table)
    }

    pub fn find_memoryidx(&self, name: &str) -> Option<u32> {
        self.find_exportidx(name, crate::module::ImportExportKind::Memory)
    }

    pub fn exported_functions(&self) -> Vec<(String, u32)> {
        self.module
            .exports
            .iter()
            .filter(|e| e.kind == crate::module::ImportExportKind::Function)
            .map(|e| (e.name.clone(), e.index))
            .collect()
    }

    pub fn exported_globals(&self) -> Vec<(String, u32)> {
        self.module
            .exports
            .iter()
            .filter(|e| e.kind == crate::module::ImportExportKind::Global)
            .map(|e| (e.name.clone(), e.index))
            .collect()
    }

    pub fn exported_tables(&self) -> Vec<(String, u32)> {
        self.module
            .exports
            .iter()
            .filter(|e| e.kind == crate::module::ImportExportKind::Table)
            .map(|e| (e.name.clone(), e.index))
            .collect()
    }

    pub fn exported_memories(&self) -> Vec<(String, u32)> {
        self.module
            .exports
            .iter()
            .filter(|e| e.kind == crate::module::ImportExportKind::Memory)
            .map(|e| (e.name.clone(), e.index))
            .collect()
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
            Type::FunctionType(crate::FuncType {
                params: vec![],
                results: frame.return_types.clone(),
            })
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
