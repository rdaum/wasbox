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

    fn read_byte(&mut self) -> Result<u8, DecodeError> {
        let mut buf = [0u8; 1];
        self.cursor.read_exact(&mut buf).map_err(|_| {
            DecodeError::MalformedMemory("unexpected end of section or function".to_string())
        })?;
        Ok(buf[0])
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
        let mut result = 0u32;
        let mut shift = 0;
        let mut byte_count = 0;

        loop {
            let byte = self.read_byte()?;
            byte_count += 1;

            // Check if we've read too many bytes (max 5 bytes for 32-bit LEB128)
            if byte_count > 5 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }

            result |= ((byte & 0x7F) as u32) << shift;

            if byte & 0x80 == 0 {
                // Final byte - check for unused bits
                let remaining_bits = 32 - shift;
                if remaining_bits < 7 {
                    let max_value = (1u8 << remaining_bits) - 1;
                    if (byte & 0x7F) > max_value {
                        return Err(DecodeError::MalformedMemory(
                            "integer too large".to_string(),
                        ));
                    }
                }
                break;
            }

            shift += 7;
            if shift >= 32 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    pub fn load_imm_varint32(&mut self) -> Result<i32, DecodeError> {
        let value = self.load_imm_varuint32()?;
        Ok(value as i32)
    }

    /// Read a signed LEB128 integer (for constants)
    pub fn load_imm_signed_varint32(&mut self) -> Result<i32, DecodeError> {
        let mut result = 0i32;
        let mut shift = 0;
        let mut byte_count = 0;

        loop {
            let byte = self.read_byte()?;
            byte_count += 1;

            // Check if we've read too many bytes (max 5 bytes for 32-bit LEB128)
            if byte_count > 5 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }

            // Extract the 7 data bits
            let value = (byte & 0x7F) as i32;
            result |= value << shift;

            // Check if this is the last byte (MSB is 0)
            if byte & 0x80 == 0 {
                // Final byte - check for unused bits
                let remaining_bits = 32 - shift;
                if remaining_bits < 7 {
                    let data_bits = byte & 0x7F;

                    // For signed LEB128, check if the sign bit would be set
                    let sign_bit_position = remaining_bits - 1;
                    let sign_bit_mask = 1u8 << sign_bit_position;
                    let sign_bit = (data_bits & sign_bit_mask) != 0;

                    if sign_bit {
                        // Negative number: unused bits above the sign bit must be 1
                        let sign_extend_mask = 0x7F & (0x7F << remaining_bits);
                        if (data_bits & sign_extend_mask) != sign_extend_mask {
                            return Err(DecodeError::MalformedMemory(
                                "integer too large".to_string(),
                            ));
                        }
                    } else {
                        // Positive number: unused bits above the sign bit must be 0
                        let unused_bits_mask = 0x7F & (0x7F << remaining_bits);
                        if (data_bits & unused_bits_mask) != 0 {
                            return Err(DecodeError::MalformedMemory(
                                "integer too large".to_string(),
                            ));
                        }
                    }
                }

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
                    "integer representation too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    /// Read a signed LEB128 long integer (for constants)
    pub fn load_imm_signed_varint64(&mut self) -> Result<i64, DecodeError> {
        let mut result = 0i64;
        let mut shift = 0;
        let mut byte_count = 0;

        loop {
            let byte = self.read_byte()?;
            byte_count += 1;

            // Check if we've read too many bytes (max 10 bytes for 64-bit LEB128)
            if byte_count > 10 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }

            // Extract the 7 data bits
            let value = (byte & 0x7F) as i64;
            result |= value << shift;

            // Check if this is the last byte (MSB is 0)
            if byte & 0x80 == 0 {
                // Final byte - check for unused bits
                let remaining_bits = 64 - shift;
                if remaining_bits < 7 {
                    let data_bits = byte & 0x7F;

                    // For signed LEB128, check if the sign bit would be set
                    let sign_bit_position = remaining_bits - 1;
                    let sign_bit_mask = 1u8 << sign_bit_position;
                    let sign_bit = (data_bits & sign_bit_mask) != 0;

                    if sign_bit {
                        // Negative number: unused bits above the sign bit must be 1
                        let sign_extend_mask = 0x7F & (0x7F << remaining_bits);
                        if (data_bits & sign_extend_mask) != sign_extend_mask {
                            return Err(DecodeError::MalformedMemory(
                                "integer too large".to_string(),
                            ));
                        }
                    } else {
                        // Positive number: unused bits above the sign bit must be 0
                        let unused_bits_mask = 0x7F & (0x7F << remaining_bits);
                        if (data_bits & unused_bits_mask) != 0 {
                            return Err(DecodeError::MalformedMemory(
                                "integer too large".to_string(),
                            ));
                        }
                    }
                }

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
                    "integer representation too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    pub fn load_imm_varuint64(&mut self) -> Result<u64, DecodeError> {
        let mut result = 0u64;
        let mut shift = 0;
        let mut byte_count = 0;

        loop {
            let byte = self.read_byte()?;
            byte_count += 1;

            // Check if we've read too many bytes (max 10 bytes for 64-bit LEB128)
            if byte_count > 10 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }

            result |= ((byte & 0x7F) as u64) << shift;

            if byte & 0x80 == 0 {
                // Final byte - check for unused bits
                let remaining_bits = 64 - shift;
                if remaining_bits < 7 {
                    let max_value = (1u8 << remaining_bits) - 1;
                    if (byte & 0x7F) > max_value {
                        return Err(DecodeError::MalformedMemory(
                            "integer too large".to_string(),
                        ));
                    }
                }
                break;
            }

            shift += 7;
            if shift >= 64 {
                return Err(DecodeError::MalformedMemory(
                    "integer representation too long".to_string(),
                ));
            }
        }

        Ok(result)
    }

    pub fn load_imm_u8(&mut self) -> Result<u8, DecodeError> {
        self.read_byte()
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
