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

use crate::exec::Fault;

#[derive(Clone, Eq, PartialEq, Copy, Debug)]
pub enum Tag {
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

/// Entries in the stack are raw u64s and are interpreted as the appropriate type when popped.
/// We could store `Value` here, but it doesn't have a u32/u64 variant, and all uses are explicitly
/// already casting to the appropriate type, anyway, so no need packing/unpacking a variant everywhere.
pub struct Stack {
    pub data: Vec<(Tag, u64)>,
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
        self.data
            .push((Tag::I32, unsafe { std::mem::transmute::<i64, u64>(value) }));
    }

    pub fn push_i64(&mut self, value: i64) {
        self.data
            .push((Tag::I64, unsafe { std::mem::transmute::<i64, u64>(value) }));
    }

    pub fn push_u32(&mut self, value: u32) {
        let value = value as u64;
        self.data.push((Tag::U32, value));
    }

    pub fn push_u64(&mut self, value: u64) {
        self.data.push((Tag::U64, value));
    }

    pub fn push_f32(&mut self, value: f32) {
        let bits = value.to_bits();
        let value = bits as u64;
        self.data.push((Tag::F32, value));
    }

    pub fn push_f64(&mut self, value: f64) {
        self.data.push((Tag::F64, value.to_bits()));
    }

    pub fn top_i32(&self) -> Result<i32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::I32);
                unsafe { std::mem::transmute(v as u32) }
            })
    }

    pub fn pop_i32(&mut self) -> Result<i32, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::I32, "Expected I32, got {:?} == {}", t, v);
            unsafe { std::mem::transmute(v as u32) }
        })
    }

    pub fn pop_i64(&mut self) -> Result<i64, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::I64);
            unsafe { std::mem::transmute(v) }
        })
    }

    pub fn top_f32(&self) -> Result<f32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::F32);
                f32::from_bits(v as u32)
            })
    }

    pub fn pop_u32(&mut self) -> Result<u32, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::U32);
            v as u32
        })
    }

    pub fn pop_untyped(&mut self) -> Result<u64, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(_, v)| v)
    }

    pub fn top_f64(&self) -> Result<f64, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::F64);
                f64::from_bits(v)
            })
    }

    pub fn top_u32(&self) -> Result<u32, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::U32);
                v as u32
            })
    }

    pub fn pop_u64(&mut self) -> Result<u64, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::U64);
            v
        })
    }

    pub fn pop_f32(&mut self) -> Result<f32, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::F32);
            f32::from_bits(v as u32)
        })
    }

    pub fn top_i64(&self) -> Result<i64, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::I64);
                unsafe { std::mem::transmute(v) }
            })
    }
    pub fn pop_f64(&mut self) -> Result<f64, Fault> {
        self.data.pop().ok_or(Fault::StackUnderflow).map(|(t, v)| {
            assert_eq!(t, Tag::F64);
            f64::from_bits(v)
        })
    }

    pub fn top_u64(&self) -> Result<u64, Fault> {
        self.data
            .last()
            .cloned()
            .ok_or(Fault::StackUnderflow)
            .map(|(t, v)| {
                assert_eq!(t, Tag::U64);
                v
            })
    }

    pub fn top_stack(&self) -> Result<(Tag, u64), Fault> {
        self.data.last().cloned().ok_or(Fault::StackUnderflow)
    }
}

#[cfg(test)]
mod tests {
    use crate::Value;

    #[test]
    fn push_pop_f32() {
        let mut stack = super::Stack::new();
        stack.push_f32(6.666);
        assert_eq!(stack.top_f32().unwrap(), 6.666);
        assert_eq!(stack.pop_f32().unwrap(), 6.666);
        // nan
        stack.push_f32(f32::NAN);
        assert!(stack.top_f32().unwrap().is_nan());
        assert!(stack.pop_f32().unwrap().is_nan());
    }

    #[test]
    fn push_pop_f64() {
        let mut stack = super::Stack::new();
        stack.push_f64(6.666);
        assert_eq!(stack.top_f64().unwrap(), 6.666);
        assert_eq!(stack.pop_f64().unwrap(), 6.666);
        // nan
        stack.push_f64(f64::NAN);
        assert!(stack.top_f64().unwrap().is_nan());
        assert!(stack.pop_f64().unwrap().is_nan());
    }

    #[test]
    fn push_pop_i32() {
        let mut stack = super::Stack::new();
        stack.push_i32(42);
        assert_eq!(stack.top_i32().unwrap(), 42);
        assert_eq!(stack.pop_i32().unwrap(), 42);

        stack.push_i32(-42);
        assert_eq!(stack.top_i32().unwrap(), -42);
        assert_eq!(stack.pop_i32().unwrap(), -42);
    }

    #[test]
    fn push_pop_i64() {
        let mut stack = super::Stack::new();
        stack.push_i64(42);
        assert_eq!(stack.top_i64().unwrap(), 42);
        assert_eq!(stack.pop_i64().unwrap(), 42);

        stack.push_i64(-42);
        assert_eq!(stack.top_i64().unwrap(), -42);
        assert_eq!(stack.pop_i64().unwrap(), -42);
    }

    #[test]
    fn push_pop_u32() {
        let mut stack = super::Stack::new();
        stack.push_u32(42);
        assert_eq!(stack.top_u32().unwrap(), 42);
        assert_eq!(stack.pop_u32().unwrap(), 42);
    }

    #[test]
    fn push_pop_u64() {
        let mut stack = super::Stack::new();
        stack.push_u64(42);
        assert_eq!(stack.top_u64().unwrap(), 42);
        assert_eq!(stack.pop_u64().unwrap(), 42);
    }

    #[test]
    fn push_pop_types() {
        let value_set = vec![
            Value::F64(42.0),
            Value::F32(42.0),
            Value::I64(42),
            Value::I32(42),
        ];
        let mut stack = super::Stack::new();
        for value in &value_set {
            value.push_to(&mut stack);
        }
        for value in value_set.iter().rev() {
            let x = Value::pop_from(value.type_of(), &mut stack).unwrap();
            assert_eq!(x, *value);
        }
    }
}
