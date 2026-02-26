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

pub struct Frame {
    pub locals: Vec<u64>,
    pub program: Rc<Program>,
    pub stack: Stack,
    pub pc: usize,
    pub control_stack: Vec<Control>,
    pub scratch_values_raw: Vec<u64>,
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
        Frame {
            locals: vec![0; num_locals],
            stack: Stack::new(),
            pc: 0,
            program,
            control_stack: Vec::with_capacity(8),
            scratch_values_raw: Vec::with_capacity(8),
        }
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
                    self.scratch_values_raw.push(Value::pop_raw_from(*vt, &mut self.stack)?);
                }
            }
            TypeSignature::Index(idx) => {
                debug_assert!((*idx as usize) < types.len());
                let ft = unsafe { types.get_unchecked(*idx as usize) };
                self.scratch_values_raw.resize(ft.results.len(), 0);
                for (i, vt) in ft.results.iter().enumerate().rev() {
                    self.scratch_values_raw[i] = Value::pop_raw_from(*vt, &mut self.stack)?;
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

    pub fn push_local_to_stack(&mut self, local_index: u32) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        let val = self.locals[local_index as usize];
        let ty = self.program.local_types[local_index as usize];
        Value::push_raw_to(ty, val, &mut self.stack);
        Ok(())
    }

    pub fn set_local_from_stack(&mut self, local_index: u32, pop: bool) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        let ty = self.program.local_types[local_index as usize];

        let val = if pop {
            Value::pop_raw_from(ty, &mut self.stack)?
        } else {
            Value::top_raw_of(ty, &mut self.stack)?
        };

        self.locals[local_index as usize] = val;
        Ok(())
    }
}
