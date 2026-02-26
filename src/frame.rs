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
use crate::{Type, ValueType};
use std::rc::Rc;

pub struct Frame {
    pub locals: Vec<Value>,
    pub program: Rc<Program>,
    pub stack: Stack,
    pub pc: usize,
    pub control_stack: Vec<Control>,
    pub scratch_values: Vec<Value>,
}

pub struct Control {
    pub signature: Type,
    pub scope_type: ScopeType,
    pub stack_width: usize,
    pub start_pc: usize,
    pub end_pc: usize,
}

impl Frame {
    pub fn new(num_locals: usize, program: Rc<Program>) -> Self {
        Frame {
            locals: vec![Value::Unit; num_locals],
            stack: Stack::new(),
            pc: 0,
            program,
            control_stack: vec![],
            scratch_values: vec![],
        }
    }

    pub fn push_control(
        &mut self,
        signature: Type,
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

    pub fn pop_control(&mut self) -> Result<Control, Fault> {
        let c = self
            .control_stack
            .pop()
            .ok_or(Fault::ControlStackUnderflow)?;

        // Pop the result values BEFORE shrinking the stack and stash in reusable scratch.
        self.scratch_values.clear();
        match &c.signature {
            Type::ValueType(vt) => {
                if *vt != ValueType::Unit {
                    self.scratch_values
                        .push(Value::pop_from(*vt, &mut self.stack)?);
                }
            }
            Type::FunctionType(ft) => {
                self.scratch_values.resize(ft.results.len(), Value::Unit);
                for (i, vt) in ft.results.iter().enumerate().rev() {
                    self.scratch_values[i] = Value::pop_from(*vt, &mut self.stack)?;
                }
            }
        }

        Ok(c)
    }

    pub fn push_local_to_stack(&mut self, local_index: u32) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        self.locals[local_index as usize].push_to(&mut self.stack);
        Ok(())
    }

    pub fn set_local_from_stack(&mut self, local_index: u32, pop: bool) -> Result<(), Fault> {
        if local_index as usize >= self.locals.len() {
            return Err(Fault::LocalIndexOutOfBounds);
        }
        let type_of_local = self.program.local_types[local_index as usize];

        let value = if pop {
            Value::pop_from(type_of_local, &mut self.stack)?
        } else {
            Value::top_of(type_of_local, &mut self.stack)?
        };

        self.locals[local_index as usize] = value;
        Ok(())
    }
}
