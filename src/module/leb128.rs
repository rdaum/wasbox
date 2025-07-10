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
use std::io::{BufRead, Cursor, Read};
use varint_rs::VarintReader;

pub struct LEB128Reader<'a> {
    cursor: Cursor<&'a [u8]>,
    len: usize,
}

impl<'a> LEB128Reader<'a> {
    pub fn new(slice: &'a [u8], start_position: usize) -> Self {
        let mut cursor = Cursor::new(slice);
        cursor.set_position(start_position as u64);
        Self {
            cursor,
            len: slice.len(),
        }
    }

    pub fn remaining(&self) -> isize {
        self.len as isize - self.cursor.position() as isize
    }

    pub fn position(&self) -> usize {
        self.cursor.position() as usize
    }

    pub fn advance(&mut self, offset: usize) {
        self.cursor.consume(offset);
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
        let start = self.cursor.position() as usize;

        let end = scan(self)? - 1;

        Ok((start, end))
    }

    pub fn load_data(&mut self) -> Result<(usize, usize), DecodeError> {
        let length = self.load_imm_varuint32()? as usize;
        let start = self.cursor.position() as usize;
        let end = start + length;
        self.cursor.consume(length);
        Ok((start, end))
    }

    pub fn load_string(&mut self) -> Result<String, DecodeError> {
        let length = self.load_imm_varuint32()? as usize;
        let mut buffer = vec![0u8; length];
        self.cursor.read_exact(&mut buffer).map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to read string of length {} at offset {}",
                length,
                self.cursor.position()
            ))
        })?;
        let string = String::from_utf8(buffer).map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode string of length {} at offset {}",
                length,
                self.cursor.position()
            ))
        })?;
        Ok(string)
    }
    pub fn load_imm_varuint32(&mut self) -> Result<u32, DecodeError> {
        self.cursor.read_u32_varint().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varuint32 at offset {}",
                self.cursor.position()
            ))
        })
    }

    pub fn load_imm_varint32(&mut self) -> Result<i32, DecodeError> {
        let value = self.load_imm_varuint32()?;
        Ok(value as i32)
    }


    /// Read a signed LEB128 integer (for constants)
    pub fn load_imm_signed_varint32(&mut self) -> Result<i32, DecodeError> {
        let mut result = 0i32;
        let mut shift = 0;

        loop {
            let byte: u8 = VarintReader::read(&mut self.cursor).map_err(|_| {
                DecodeError::MalformedMemory(format!(
                    "Failed to read byte for signed varint32 at offset {}",
                    self.cursor.position()
                ))
            })?;

            // Extract the 7 data bits
            let value = (byte & 0x7F) as i32;
            result |= value << shift;

            // Check if this is the last byte (MSB is 0)
            if byte & 0x80 == 0 {
                // Sign extend if necessary
                if shift < 32 && (byte & 0x40) != 0 {
                    let sign_extend_shift = shift + 7;
                    if sign_extend_shift < 32 {
                        result |= !0 << sign_extend_shift;
                    }
                }
                break;
            }

            shift += 7;
            if shift >= 32 {
                return Err(DecodeError::MalformedMemory(
                    "Signed varint32 too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    /// Read a signed LEB128 long integer (for constants)
    pub fn load_imm_signed_varint64(&mut self) -> Result<i64, DecodeError> {
        let mut result = 0i64;
        let mut shift = 0;

        loop {
            let byte: u8 = VarintReader::read(&mut self.cursor).map_err(|_| {
                DecodeError::MalformedMemory(format!(
                    "Failed to read byte for signed varint64 at offset {}",
                    self.cursor.position()
                ))
            })?;

            // Extract the 7 data bits
            let value = (byte & 0x7F) as i64;
            result |= value << shift;

            // Check if this is the last byte (MSB is 0)
            if byte & 0x80 == 0 {
                // Sign extend if necessary
                if shift < 64 && (byte & 0x40) != 0 {
                    let sign_extend_shift = shift + 7;
                    if sign_extend_shift < 64 {
                        result |= !0 << sign_extend_shift;
                    }
                }
                break;
            }

            shift += 7;
            if shift >= 64 {
                return Err(DecodeError::MalformedMemory(
                    "Signed varint64 too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    pub fn load_imm_varuint64(&mut self) -> Result<u64, DecodeError> {
        self.cursor.read_u64_varint().map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode varuint64 at offset {}",
                self.cursor.position()
            ))
        })
    }

    pub fn load_imm_u8(&mut self) -> Result<u8, DecodeError> {
        let byte: u8 = VarintReader::read(&mut self.cursor).map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode u8 at offset {}",
                self.cursor.position()
            ))
        })?;
        Ok(byte)
    }
    pub fn load_imm_f32(&mut self) -> Result<f32, DecodeError> {
        let mut f32_buffer = [0u8; 4];
        self.cursor.read_exact(&mut f32_buffer).map_err(|_| {
            DecodeError::MalformedMemory(format!(
                "Failed to decode f32 at offset {}",
                self.cursor.position()
            ))
        })?;
        Ok(f32::from_le_bytes(f32_buffer))
    }

    pub fn load_imm_f64(&mut self) -> f64 {
        let mut f64_buffer = [0u8; 8];
        self.cursor.read_exact(&mut f64_buffer).unwrap();
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
