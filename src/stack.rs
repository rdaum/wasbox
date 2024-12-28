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
        self.data
            .push(unsafe { std::mem::transmute::<i64, u64>(value) });
    }

    pub fn push_i64(&mut self, value: i64) {
        self.data
            .push(unsafe { std::mem::transmute::<i64, u64>(value) });
    }

    pub fn push_u32(&mut self, value: u32) {
        let value = value as u64;
        self.data.push(value);
    }

    pub fn push_u64(&mut self, value: u64) {
        self.data.push(value);
    }

    pub fn push_f32(&mut self, value: f32) {
        let value = value as f64;
        self.data.push(value.to_bits());
    }

    pub fn push_f64(&mut self, value: f64) {
        self.data.push(value.to_bits());
    }

    pub fn top_i32(&self) -> i32 {
        let top: i64 = unsafe { std::mem::transmute(self.data.last().unwrap()) };
        top as i32
    }

    pub fn pop_i32(&mut self) -> i32 {
        let top: i64 = unsafe { std::mem::transmute(self.data.pop().unwrap()) };
        top as i32
    }

    pub fn pop_i64(&mut self) -> i64 {
        unsafe { std::mem::transmute(self.data.pop().unwrap()) }
    }

    pub fn top_f32(&self) -> f32 {
        let top: f64 = unsafe { std::mem::transmute(self.data.last().unwrap()) };
        top as f32
    }

    pub fn pop_u32(&mut self) -> u32 {
        let top = self.data.pop().unwrap();
        top as u32
    }

    pub fn top_f64(&self) -> f64 {
        unsafe { std::mem::transmute(self.data.last().unwrap()) }
    }
    pub fn top_u32(&self) -> u32 {
        let top: u64 = unsafe { std::mem::transmute(self.data.last().unwrap()) };
        top as u32
    }

    pub fn pop_u64(&mut self) -> u64 {
        self.data.pop().unwrap()
    }

    pub fn pop_f32(&mut self) -> f32 {
        f32::from_bits(self.data.pop().unwrap() as u32)
    }

    pub fn top_i64(&self) -> i64 {
        unsafe { std::mem::transmute(self.data.last().unwrap()) }
    }
    pub fn pop_f64(&mut self) -> f64 {
        f64::from_bits(self.data.pop().unwrap())
    }

    pub fn top_u64(&self) -> u64 {
        unsafe { std::mem::transmute(self.data.last().unwrap()) }
    }
}
