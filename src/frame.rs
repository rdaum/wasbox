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

use crate::decode::{LabelId, Program, ScopeType};
use crate::exec::Value;
use crate::stack::Stack;
use crate::ValueType;

pub struct Frame {
    pub locals: Vec<Value>,
    pub program: Program,
    pub stack: Stack,
    pub pc: usize,
    pub control_stack: Vec<Control>,
}

pub struct Control {
    pub signature: ValueType,
    pub scope_type: ScopeType,
    pub stack_width: usize,
    pub label: LabelId,
}

impl Frame {
    pub fn new(num_locals: usize, program: Program) -> Self {
        Frame {
            locals: vec![Value::Unit; num_locals],
            stack: Stack::new(),
            pc: 0,
            program,
            control_stack: vec![],
        }
    }

    pub fn push_control(&mut self, signature: ValueType, scope_type: ScopeType, label: LabelId) {
        self.control_stack.push(Control {
            signature,
            scope_type,
            stack_width: self.stack.width(),
            label,
        });
    }

    pub fn pop_control(&mut self) -> Control {
        let c = self.control_stack.pop().unwrap();
        self.stack.shrink_to(c.stack_width);
        c
    }

    pub fn jump_label(&mut self, label_id: LabelId) -> bool {
        // Find the label in the program's label map
        let label = self.program.labels.find_label(label_id);
        match label {
            Some(position) => {
                self.pc = position;
                true
            }
            None => false,
        }
    }

    pub fn push_local_to_stack(&mut self, local_index: u32) {
        self.locals[local_index as usize].push_to(&mut self.stack);
    }

    pub fn set_local_from_stack(&mut self, local_index: u32, pop: bool) {
        let type_of_local = self.program.local_types[local_index as usize];

        let value = if pop {
            Value::pop_to(type_of_local, &mut self.stack)
        } else {
            Value::top_to(type_of_local, &mut self.stack)
        };
        self.locals[local_index as usize] = value;
    }
}
