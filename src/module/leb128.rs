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

use crate::decode::scan;
use crate::DecodeError;
use wasabi_leb128::ReadLeb128;

pub struct LEB128Reader<'a> {
    slice: &'a [u8],
    position: usize,
}

impl<'a> LEB128Reader<'a> {
    pub fn new(slice: &'a [u8], start_position: usize) -> Self {
        Self {
            slice,
            position: start_position,
        }
    }

    pub fn remaining(&self) -> isize {
        self.slice.len() as isize - self.position as isize
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn advance(&mut self, offset: usize) {
        self.position += offset;
    }
}

impl LEB128Reader<'_> {
    /// Return the start and end (exclusive) of the expression and update the position to after
    /// the expression
    pub fn load_expr(&mut self) -> Result<(usize, usize), DecodeError> {
        // Expr is set of opcodes terminated by 0x0b, so we just need to find the next 0x0b
        // And then return the offsets of the start and end (exclusive) of the expression
        // What's tricky here is we actually need to decode the instruction stream and their
        // arguments in order to manage the block stack.
        // It's not enough to just look for a termination 0x0b, because we need to know if it's
        // a block, loop, if, etc. and then manage the block stack accordingly.
        // And this also means not decoding arguments to instructions as instructions.
        // So really it's a full decode, but without the actual translation.
        let start = self.position;
        scan(self)?;
        let end = self.position;
        Ok((start, end))
    }

    pub fn load_data(&mut self) -> Result<(usize, usize), DecodeError> {
        let length = self.load_imm_varuint32()? as usize;
        let start = self.position;
        let end = start + length;
        self.advance(length);
        Ok((start, end))
    }

    pub fn load_string(&mut self) -> Result<String, DecodeError> {
        let length = self.load_imm_varuint32()? as usize;
        let mut buffer = vec![0u8; length];
        let read = self
            .slice
            .get(self.position..self.position + length)
            .ok_or_else(|| {
                DecodeError::MalformedMemory(format!(
                    "Failed to read string of length {} at offset {}",
                    length, self.position
                ))
            })?;
        buffer.copy_from_slice(read);
        self.advance(length);
        let string = String::from_utf8(buffer).map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode string of length {} at offset {}",
                length, self.position
            ))
        })?;
        Ok(string)
    }
    pub fn load_imm_varuint32(&mut self) -> Result<u32, DecodeError> {
        let mut slice = &self.slice[self.position..];
        let (value, bytes_read) = slice.read_leb128().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varuint32 at offset {}",
                self.position
            ))
        })?;
        self.advance(bytes_read);
        Ok(value)
    }

    pub fn load_imm_varint32(&mut self) -> Result<i32, DecodeError> {
        let mut slice = &self.slice[self.position..];
        let (value, bytes_read): (i32, usize) = slice.read_leb128().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varint32 at offset {}",
                self.position
            ))
        })?;
        self.advance(bytes_read);
        Ok(value)
    }

    #[allow(dead_code)]
    pub fn load_imm_varint8(&mut self) -> Result<i8, DecodeError> {
        let mut slice = &self.slice[self.position..];
        let (value, bytes_read): (i8, usize) = slice.read_leb128().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varint8 at offset {}",
                self.position
            ))
        })?;
        self.advance(bytes_read);
        Ok(value)
    }

    pub fn load_imm_varint64(&mut self) -> Result<i64, DecodeError> {
        let mut slice = &self.slice[self.position..];
        let (value, bytes_read): (i64, usize) = slice.read_leb128().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varint64 at offset {}",
                self.position
            ))
        })?;
        self.advance(bytes_read);
        Ok(value)
    }

    pub fn load_imm_varuint64(&mut self) -> Result<u64, DecodeError> {
        let mut slice = &self.slice[self.position..];
        let (value, bytes_read) = slice.read_leb128().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varuint64 at offset {}",
                self.position
            ))
        })?;
        self.advance(bytes_read);
        Ok(value)
    }

    pub fn load_imm_u8(&mut self) -> Result<u8, DecodeError> {
        let value = self.slice.get(self.position).ok_or_else(|| {
            DecodeError::MalformedMemory(format!("Failed to decode u8 at offset {}", self.position))
        })?;
        self.advance(1);
        Ok(*value)
    }
    pub fn load_imm_f32(&mut self) -> Result<f32, DecodeError> {
        let mut f32_buffer = [0u8; 4];
        f32_buffer.copy_from_slice(&self.slice[self.position..self.position + 4]);
        self.advance(4);
        Ok(f32::from_le_bytes(f32_buffer))
    }

    pub fn load_imm_f64(&mut self) -> f64 {
        let mut f64_buffer = [0u8; 8];
        f64_buffer.copy_from_slice(&self.slice[self.position..self.position + 8]);
        self.advance(8);
        f64::from_le_bytes(f64_buffer)
    }

    #[allow(dead_code)]
    pub fn load_array_i32(&mut self) -> Result<Vec<i32>, DecodeError> {
        let num_elements = self.load_imm_varuint32()? as usize;
        let mut values = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            values.push(self.load_imm_varint32()?);
        }
        Ok(values)
    }

    pub fn load_array_varu32(&mut self) -> Result<Vec<u32>, DecodeError> {
        let num_elements = self.load_imm_varuint32()? as usize;
        let mut values = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            values.push(self.load_imm_varuint32()?);
        }
        Ok(values)
    }
}
