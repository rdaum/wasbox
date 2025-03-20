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

use crate::decode::{Label, LabelId, Program, ScopeType};
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
    pub label: LabelId,
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

    pub fn push_control(&mut self, signature: Type, scope_type: ScopeType, label: LabelId) {
        eprintln!(
            "Pushing control frame: {:?}, stack is width: {}",
            scope_type,
            self.stack.width()
        );
        let result_width = match &signature {
            Type::ValueType(ValueType::Unit) => 0,
            Type::ValueType(_) => 1,
            Type::FunctionType(i) => i.results.len(),
        };
        self.control_stack.push(Control {
            signature,
            scope_type,
            stack_width: self.stack.width() + result_width,
            label,
        });
    }

    pub fn pop_control(&mut self) -> Result<Control, Fault> {
        let c = self.control_stack.pop().ok_or_else(|| {
            println!("Control stack underflow");
            Fault::ControlStackUnderflow
        })?;
        eprintln!("Popped control frame: {:?}", c.scope_type);
        // Ensure that the value stack is the same width as when the control frame was pushed.
        self.stack.truncate(c.stack_width);
        eprintln!("Stack now: {:?}", self.stack.data);
        Ok(c)
    }

    pub fn jump_label(&mut self, label_id: LabelId) -> bool {
        // Find the label in the program's label map
        let label = self.program.find_label(label_id).cloned();
        match label {
            Some(Label::Bound {
                position,
                control_stack_depth,
            }) => {
                eprintln!(
                    "Jumping to label, position: {}, target control_stack_depth: {}",
                    position, control_stack_depth
                );
                self.pc = position;
                // Pop the control stack until we reach the target depth
                while self.control_stack.len() > control_stack_depth {
                    self.pop_control().unwrap();
                }
                true
            }
            _ => {
                println!("Unbound label: {:?}", label_id);
                false
            }
        }
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
