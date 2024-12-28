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


pub struct Memory<'a> {
    data: &'a mut [u8],
}
impl<'a> Memory<'a> {
    pub fn new(data: &'a mut [u8]) -> Self {
        Memory { data }
    }
    pub fn data(&self) -> &[u8] {
        self.data
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn get_u8(&self, offset: usize) -> u8 {
        self.data()[offset]
    }

    pub fn get_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data()[offset], self.data()[offset + 1]])
    }

    pub fn get_i32(&self, offset: usize) -> i32 {
        i32::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
        ])
    }

    pub fn get_i64(&self, offset: usize) -> i64 {
        i64::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
            self.data()[offset + 4],
            self.data()[offset + 5],
            self.data()[offset + 6],
            self.data()[offset + 7],
        ])
    }

    pub fn get_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
        ])
    }

    pub fn get_u64(&self, offset: usize) -> u64 {
        u64::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
            self.data()[offset + 4],
            self.data()[offset + 5],
            self.data()[offset + 6],
            self.data()[offset + 7],
        ])
    }

    pub fn get_f32(&self, offset: usize) -> f32 {
        f32::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
        ])
    }

    pub fn get_f64(&self, offset: usize) -> f64 {
        f64::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
            self.data()[offset + 4],
            self.data()[offset + 5],
            self.data()[offset + 6],
            self.data()[offset + 7],
        ])
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        self.data
    }

    pub fn set_u8(&mut self, offset: usize, value: u8) {
        self.data_mut()[offset] = value;
    }

    pub fn set_u16(&mut self, offset: usize, value: u16) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
    }

    pub fn set_i32(&mut self, offset: usize, value: i32) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
    }

    pub fn set_i64(&mut self, offset: usize, value: i64) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        self.data_mut()[offset + 4] = le[4];
        self.data_mut()[offset + 5] = le[5];
        self.data_mut()[offset + 6] = le[6];
        self.data_mut()[offset + 7] = le[7];
    }

    pub fn set_u32(&mut self, offset: usize, value: u32) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
    }

    pub fn set_u64(&mut self, offset: usize, value: u64) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        self.data_mut()[offset + 4] = le[4];
        self.data_mut()[offset + 5] = le[5];
        self.data_mut()[offset + 6] = le[6];
        self.data_mut()[offset + 7] = le[7];
    }

    pub fn set_f32(&mut self, offset: usize, value: f32) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
    }

    pub fn set_f64(&mut self, offset: usize, value: f64) {
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        self.data_mut()[offset + 4] = le[4];
        self.data_mut()[offset + 5] = le[5];
        self.data_mut()[offset + 6] = le[6];
        self.data_mut()[offset + 7] = le[7];
    }
}
