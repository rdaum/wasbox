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

use crate::exec::{Fault, Value};

/// Entries in the stack are raw u64s and are interpreted as the appropriate type when popped.
/// We could store `Value` here, but it doesn't have a u32/u64 variant, and all uses are explicitly
/// already casting to the appropriate type, anyway, so no need packing/unpacking a variant everywhere.
#[derive(Debug)]
pub struct Stack {
    data: Vec<u64>,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    pub fn new() -> Self {
        Stack { data: vec![] }
    }

    pub fn width(&self) -> usize {
        self.data.len()
    }

    pub fn shrink_to(&mut self, width: usize) {
        self.data.truncate(width);
    }
}
impl Stack {
    pub fn push_i32(&mut self, value: i32) {
        let value = value as i64;
        self.data.push(value as u64);
    }

    pub fn push_i64(&mut self, value: i64) {
        self.data.push(value as u64);
    }

    pub fn push_u32(&mut self, value: u32) {
        let value = value as u64;
        self.data.push(value);
    }

    pub fn push_u64(&mut self, value: u64) {
        self.data.push(value);
    }

    pub fn push_f32(&mut self, value: f32) {
        let bits = value.to_bits();
        let value = bits as u64;
        self.data.push(value);
    }

    pub fn push_f64(&mut self, value: f64) {
        self.data.push(value.to_bits());
    }

    pub fn top_i32(&self) -> Result<i32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as u32 as i32)
    }

    pub fn pop_i32(&mut self) -> Result<i32, Fault> {
        self.data
            .pop()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as u32 as i32)
    }

    pub fn pop_i64(&mut self) -> Result<i64, Fault> {
        self.data
            .pop()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as i64)
    }

    pub fn top_f32(&self) -> Result<f32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|v| f32::from_bits(v as u32))
    }

    pub fn pop_u32(&mut self) -> Result<u32, Fault> {
        self.data
            .pop()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as u32)
    }

    pub fn top_f64(&self) -> Result<f64, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(f64::from_bits)
    }

    pub fn top_u32(&self) -> Result<u32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as u32)
    }

    pub fn pop_u64(&mut self) -> Result<u64, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow)
    }

    pub fn pop_f32(&mut self) -> Result<f32, Fault> {
        self.data
            .pop()
            .ok_or(Fault::StackUnderflow)
            .map(|v| f32::from_bits(v as u32))
    }

    pub fn top_i64(&self) -> Result<i64, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|v| v as i64)
    }
    pub fn pop_f64(&mut self) -> Result<f64, Fault> {
        self.data
            .pop()
            .ok_or(Fault::StackUnderflow)
            .map(f64::from_bits)
    }

    pub fn push_ref(&mut self, value: Option<u32>) {
        // Use u32::MAX to represent None, otherwise store the value
        let encoded = match value {
            Some(v) => v as u64,
            None => u32::MAX as u64,
        };
        self.data.push(encoded);
    }

    pub fn pop_ref(&mut self) -> Result<Option<u32>, Fault> {
        let raw = self.data.pop().ok_or(Fault::StackUnderflow)?;
        if raw == u32::MAX as u64 {
            Ok(None)
        } else {
            Ok(Some(raw as u32))
        }
    }

    pub fn pop_value(&mut self) -> Result<Value, Fault> {
        let raw = self.data.pop().ok_or(Fault::StackUnderflow)?;
        // For now, assume it's an i32 (could be improved to track types)
        Ok(Value::I32(raw as i32))
    }

    pub fn top_u64(&self) -> Result<u64, Fault> {
        self.data.last().cloned().ok_or(Fault::StackUnderflow)
    }
}
