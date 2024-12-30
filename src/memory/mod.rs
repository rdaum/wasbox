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

pub use slice_mem::SliceMemory;
pub use vector_mem::VectorMemory;

mod slice_mem;
mod vector_mem;

// TODO: MmapMemory, both file and anonymous

pub trait Memory {
    fn data(&self) -> &[u8];
    fn data_mut(&mut self) -> &mut [u8];

    fn size(&self) -> usize;
    fn grow(&mut self, _new_size: usize) -> Result<usize, Fault>;
    fn get_u8(&self, offset: usize) -> Result<u8, Fault> {
        if offset >= self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(self.data()[offset])
    }
    fn get_u16(&self, offset: usize) -> Result<u16, Fault> {
        if offset + 2 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(u16::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
        ]))
    }
    fn get_i32(&self, offset: usize) -> Result<i32, Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(i32::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
        ]))
    }
    fn get_i64(&self, offset: usize) -> Result<i64, Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(i64::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
            self.data()[offset + 4],
            self.data()[offset + 5],
            self.data()[offset + 6],
            self.data()[offset + 7],
        ]))
    }
    fn get_u32(&self, offset: usize) -> Result<u32, Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(u32::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
        ]))
    }
    fn get_u64(&self, offset: usize) -> Result<u64, Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        Ok(u64::from_le_bytes([
            self.data()[offset],
            self.data()[offset + 1],
            self.data()[offset + 2],
            self.data()[offset + 3],
            self.data()[offset + 4],
            self.data()[offset + 5],
            self.data()[offset + 6],
            self.data()[offset + 7],
        ]))
    }
    fn get_f32(&self, offset: usize) -> Result<f32, Fault> {
        let u = self.get_u32(offset)?;
        Ok(f32::from_bits(u))
    }

    fn get_f64(&self, offset: usize) -> Result<f64, Fault> {
        let u = self.get_u64(offset)?;
        Ok(f64::from_bits(u))
    }

    fn set_u8(&mut self, offset: usize, value: u8) -> Result<(), Fault> {
        if offset >= self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        self.data_mut()[offset] = value;
        Ok(())
    }
    fn set_u16(&mut self, offset: usize, value: u16) -> Result<(), Fault> {
        if offset + 2 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        Ok(())
    }
    fn set_i32(&mut self, offset: usize, value: i32) -> Result<(), Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        Ok(())
    }
    fn set_i64(&mut self, offset: usize, value: i64) -> Result<(), Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        self.data_mut()[offset + 4] = le[4];
        self.data_mut()[offset + 5] = le[5];
        self.data_mut()[offset + 6] = le[6];
        self.data_mut()[offset + 7] = le[7];
        Ok(())
    }
    fn set_u32(&mut self, offset: usize, value: u32) -> Result<(), Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        Ok(())
    }
    fn set_u64(&mut self, offset: usize, value: u64) -> Result<(), Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let le = value.to_le_bytes();
        self.data_mut()[offset] = le[0];
        self.data_mut()[offset + 1] = le[1];
        self.data_mut()[offset + 2] = le[2];
        self.data_mut()[offset + 3] = le[3];
        self.data_mut()[offset + 4] = le[4];
        self.data_mut()[offset + 5] = le[5];
        self.data_mut()[offset + 6] = le[6];
        self.data_mut()[offset + 7] = le[7];
        Ok(())
    }
    fn set_f32(&mut self, offset: usize, value: f32) -> Result<(), Fault> {
        let value = value.to_bits();
        self.set_u32(offset, value)
    }

    fn set_f64(&mut self, offset: usize, value: f64) -> Result<(), Fault> {
        let value = value.to_bits();
        self.set_u64(offset, value)
    }
}
