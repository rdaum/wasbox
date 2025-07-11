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

pub struct Frame {
    pub locals: Vec<Value>,
    pub return_types: Vec<ValueType>,
    pub program: Program,
    pub stack: Stack,
    pub pc: usize,
    pub control_stack: Vec<Control>,
}

pub struct Control {
    pub signature: Type,
    pub scope_type: ScopeType,
    pub stack_width: usize,
}

impl Frame {
    pub fn new(num_locals: usize, program: Program) -> Self {
        let return_types = program.return_types.clone();
        Frame {
            locals: vec![Value::Unit; num_locals],
            stack: Stack::new(),
            pc: 0,
            program,
            control_stack: vec![],
            return_types,
        }
    }

    pub fn push_control(&mut self, signature: Type, scope_type: ScopeType) {
        self.control_stack.push(Control {
            signature,
            scope_type,
            stack_width: self.stack.width(),
        });
    }

    pub fn pop_control(&mut self) -> Result<(Control, Vec<Value>), Fault> {
        let c = self
            .control_stack
            .pop()
            .ok_or(Fault::ControlStackUnderflow)?;

        // Pop the result values BEFORE shrinking the stack
        let results = match &c.signature {
            Type::ValueType(vt) => {
                if *vt != ValueType::Unit {
                    vec![Value::pop_from(*vt, &mut self.stack)?]
                } else {
                    vec![]
                }
            }
            Type::FunctionType(ft) => {
                // Pre-allocate and assign by index to avoid double-reverse
                let mut results = vec![Value::Unit; ft.results.len()];
                for (i, vt) in ft.results.iter().enumerate().rev() {
                    results[i] = Value::pop_from(*vt, &mut self.stack)?;
                }
                results
            }
        };

        // Note: We don't shrink the stack here - that's the caller's responsibility
        // The caller can decide whether to shrink the stack and push results back

        Ok((c, results))
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
