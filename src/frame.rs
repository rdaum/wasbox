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

use crate::decode::{Program, ScopeType};
use crate::exec::{Fault, Value};
use crate::stack::Stack;
use crate::{FuncType, TypeSignature, ValueType};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProgramKey {
    Local { owner_id: u32, local_idx: u32 },
    Ephemeral,
}

impl ProgramKey {
    #[inline(always)]
    pub const fn local(owner_id: u32, local_idx: u32) -> Self {
        Self::Local {
            owner_id,
            local_idx,
        }
    }

    #[inline(always)]
    pub const fn ephemeral() -> Self {
        Self::Ephemeral
    }
}

#[derive(Clone, Copy)]
pub struct CallIndirectCacheLine {
    pub valid: bool,
    pub table_idx: u32,
    pub elem_idx: u32,
    pub expected_type_idx: u32,
    pub table_generation: u32,
    pub target_module_id: u32,
    pub target_func_idx: u32,
}

impl CallIndirectCacheLine {
    #[inline(always)]
    pub const fn invalid() -> Self {
        Self {
            valid: false,
            table_idx: 0,
            elem_idx: 0,
            expected_type_idx: 0,
            table_generation: 0,
            target_module_id: 0,
            target_func_idx: 0,
        }
    }

    #[inline(always)]
    pub fn matches(
        &self,
        table_idx: u32,
        elem_idx: u32,
        expected_type_idx: u32,
        table_generation: u32,
    ) -> bool {
        self.valid
            && self.table_idx == table_idx
            && self.elem_idx == elem_idx
            && self.expected_type_idx == expected_type_idx
            && self.table_generation == table_generation
    }
}

#[derive(Clone, Copy)]
pub struct CallIndirectCacheEntry {
    pub primary: CallIndirectCacheLine,
    pub secondary: CallIndirectCacheLine,
}

impl CallIndirectCacheEntry {
    #[inline(always)]
    pub const fn invalid() -> Self {
        Self {
            primary: CallIndirectCacheLine::invalid(),
            secondary: CallIndirectCacheLine::invalid(),
        }
    }
}

pub struct Frame {
    pub locals: Vec<u64>,
    pub program_key: ProgramKey,
    program_ptr: *const Program,
    owned_program: Option<Rc<Program>>,
    pub stack: Stack,
    pub pc: usize,
    pub control_stack: Vec<Control>,
    pub scratch_values_raw: Vec<u64>,
    pub call_indirect_cache: Vec<CallIndirectCacheEntry>,
}

pub struct Control {
    pub signature: TypeSignature,
    pub scope_type: ScopeType,
    pub stack_width: usize,
    pub start_pc: usize,
    pub end_pc: usize,
}

impl Frame {
    pub fn new(num_locals: usize, program: Rc<Program>) -> Self {
        Self::for_owned_program(vec![0; num_locals], program)
    }

    pub fn for_borrowed_program(
        locals: Vec<u64>,
        program_key: ProgramKey,
        program: &Rc<Program>,
    ) -> Self {
        Frame {
            locals,
            program_key,
            program_ptr: Rc::as_ptr(program),
            owned_program: None,
            stack: Stack::new(),
            pc: 0,
            control_stack: Vec::with_capacity(8),
            scratch_values_raw: Vec::with_capacity(8),
            call_indirect_cache: Vec::with_capacity(4),
        }
    }

    pub fn for_owned_program(locals: Vec<u64>, program: Rc<Program>) -> Self {
        let program_ptr = Rc::as_ptr(&program);
        Frame {
            locals,
            program_key: ProgramKey::ephemeral(),
            program_ptr,
            owned_program: Some(program),
            stack: Stack::new(),
            pc: 0,
            control_stack: Vec::with_capacity(8),
            scratch_values_raw: Vec::with_capacity(8),
            call_indirect_cache: Vec::with_capacity(4),
        }
    }

    #[inline(always)]
    pub fn program(&self) -> &Program {
        // SAFETY: `program_ptr` points to either:
        // - an `Rc<Program>` owned by the enclosing `Instance` (local frames), or
        // - `owned_program` in this frame (ephemeral frames).
        unsafe { &*self.program_ptr }
    }

    #[inline(always)]
    pub fn program_ptr(&self) -> *const Program {
        self.program_ptr
    }

    #[inline(always)]
    pub fn set_borrowed_program(&mut self, program_key: ProgramKey, program: &Rc<Program>) {
        self.owned_program = None;
        self.set_program_ptr(program_key, Rc::as_ptr(program));
    }

    #[inline(always)]
    pub fn set_owned_program(&mut self, program: Rc<Program>) {
        let program_ptr = Rc::as_ptr(&program);
        self.owned_program = Some(program);
        self.set_program_ptr(ProgramKey::ephemeral(), program_ptr);
    }

    #[inline(always)]
    fn set_program_ptr(&mut self, program_key: ProgramKey, program_ptr: *const Program) {
        if self.program_key != program_key || self.program_ptr != program_ptr {
            self.call_indirect_cache.clear();
        }
        self.program_key = program_key;
        self.program_ptr = program_ptr;
    }

    #[inline(always)]
    pub fn push_control(
        &mut self,
        signature: TypeSignature,
        scope_type: ScopeType,
        start_pc: usize,
        end_pc: usize,
    ) {
        self.control_stack.push(Control {
            signature,
            scope_type,
            stack_width: self.stack.width(),
            start_pc,
            end_pc,
        });
    }

    #[inline(always)]
    pub fn pop_control(&mut self, types: &[FuncType]) -> Result<Control, Fault> {
        let c = self
            .control_stack
            .pop()
            .ok_or(Fault::ControlStackUnderflow)?;

        // Pop the result values BEFORE shrinking the stack and stash in reusable scratch.
        // We use raw u64 storage to avoid Value overhead.
        self.scratch_values_raw.clear();
        match &c.signature {
            TypeSignature::ValueType(vt) => {
                if *vt != ValueType::Unit {
                    debug_assert_ne!(*vt, ValueType::V128);
                    self.scratch_values_raw.push(self.stack.pop_u64()?);
                }
            }
            TypeSignature::Index(idx) => {
                debug_assert!((*idx as usize) < types.len());
                let ft = unsafe { types.get_unchecked(*idx as usize) };
                self.scratch_values_raw.resize(ft.results.len(), 0);
                for (i, vt) in ft.results.iter().enumerate().rev() {
                    debug_assert_ne!(*vt, ValueType::V128);
                    self.scratch_values_raw[i] = self.stack.pop_u64()?;
                }
            }
        }

        Ok(c)
    }

    #[inline(always)]
    pub fn reset_for_function(&mut self, signature: TypeSignature, end_pc: usize) {
        self.stack.shrink_to(0);
        self.pc = 0;
        self.scratch_values_raw.clear();
        let function_control = Control {
            signature,
            scope_type: ScopeType::Function,
            stack_width: 0,
            start_pc: 0,
            end_pc,
        };
        if self.control_stack.is_empty() {
            self.control_stack.push(function_control);
        } else {
            self.control_stack[0] = function_control;
            self.control_stack.truncate(1);
        }
    }

    #[inline(always)]
    pub fn call_indirect_cache_entry_mut(
        &mut self,
        call_site_pc: usize,
    ) -> &mut CallIndirectCacheEntry {
        if call_site_pc >= self.call_indirect_cache.len() {
            self.call_indirect_cache
                .resize(call_site_pc + 1, CallIndirectCacheEntry::invalid());
        }
        &mut self.call_indirect_cache[call_site_pc]
    }

    #[inline(always)]
    pub fn lookup_call_indirect_cache(
        &mut self,
        call_site_pc: usize,
        table_idx: u32,
        elem_idx: u32,
        expected_type_idx: u32,
        table_generation: u32,
    ) -> Option<(u32, u32)> {
        let cache = self.call_indirect_cache_entry_mut(call_site_pc);
        if cache
            .primary
            .matches(table_idx, elem_idx, expected_type_idx, table_generation)
        {
            return Some((
                cache.primary.target_module_id,
                cache.primary.target_func_idx,
            ));
        }
        if cache
            .secondary
            .matches(table_idx, elem_idx, expected_type_idx, table_generation)
        {
            return Some((
                cache.secondary.target_module_id,
                cache.secondary.target_func_idx,
            ));
        }
        None
    }

    #[inline(always)]
    pub fn update_call_indirect_cache(
        &mut self,
        call_site_pc: usize,
        table_idx: u32,
        elem_idx: u32,
        expected_type_idx: u32,
        table_generation: u32,
        target_module_id: u32,
        target_func_idx: u32,
    ) {
        let new_line = CallIndirectCacheLine {
            valid: true,
            table_idx,
            elem_idx,
            expected_type_idx,
            table_generation,
            target_module_id,
            target_func_idx,
        };
        let cache = self.call_indirect_cache_entry_mut(call_site_pc);
        if !cache.primary.valid
            || (cache.primary.table_idx == table_idx
                && cache.primary.expected_type_idx == expected_type_idx
                && cache.primary.elem_idx == elem_idx)
        {
            cache.primary = new_line;
            return;
        }
        if !cache.secondary.valid
            || (cache.secondary.table_idx == table_idx
                && cache.secondary.expected_type_idx == expected_type_idx
                && cache.secondary.elem_idx == elem_idx)
        {
            cache.secondary = new_line;
            return;
        }
        // Tiny 2-way cache: evict secondary on conflict.
        cache.secondary = new_line;
    }

    pub fn push_local_to_stack(&mut self, local_index: u32) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        let val = self.locals[local_index as usize];
        let ty = self.program().local_types[local_index as usize];
        Value::push_raw_to(ty, val, &mut self.stack);
        Ok(())
    }

    pub fn set_local_from_stack(&mut self, local_index: u32, pop: bool) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        let ty = self.program().local_types[local_index as usize];

        let val = if pop {
            Value::pop_raw_from(ty, &mut self.stack)?
        } else {
            Value::top_raw_of(ty, &mut self.stack)?
        };

        self.locals[local_index as usize] = val;
        Ok(())
    }
}
