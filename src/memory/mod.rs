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
        let bytes = value.to_le_bytes();
        self.data_mut()[offset..offset + 2].copy_from_slice(&bytes);
        Ok(())
    }
    fn set_i32(&mut self, offset: usize, value: i32) -> Result<(), Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let bytes = value.to_le_bytes();
        self.data_mut()[offset..offset + 4].copy_from_slice(&bytes);
        Ok(())
    }
    fn set_i64(&mut self, offset: usize, value: i64) -> Result<(), Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let bytes = value.to_le_bytes();
        self.data_mut()[offset..offset + 8].copy_from_slice(&bytes);
        Ok(())
    }
    fn set_u32(&mut self, offset: usize, value: u32) -> Result<(), Fault> {
        if offset + 4 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let bytes = value.to_le_bytes();
        self.data_mut()[offset..offset + 4].copy_from_slice(&bytes);
        Ok(())
    }
    fn set_u64(&mut self, offset: usize, value: u64) -> Result<(), Fault> {
        if offset + 8 > self.size() {
            return Err(Fault::MemoryOutOfBounds);
        }
        let bytes = value.to_le_bytes();
        self.data_mut()[offset..offset + 8].copy_from_slice(&bytes);
        Ok(())
    }
    fn set_f32(&mut self, offset: usize, value: f32) -> Result<(), Fault> {
        self.set_u32(offset, value.to_bits())
    }
    fn set_f64(&mut self, offset: usize, value: f64) -> Result<(), Fault> {
        self.set_u64(offset, value.to_bits())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::VectorMemory;

    #[test]
    fn test_vector_memory_creation() {
        let memory = VectorMemory::new(1024, Some(2048));
        assert_eq!(memory.size(), 1024);
        assert_eq!(memory.data().len(), 1024);

        // Memory should be zero-initialized
        for byte in memory.data() {
            assert_eq!(*byte, 0);
        }
    }

    #[test]
    fn test_vector_memory_wasm_page_size() {
        const WASM_PAGE_SIZE: usize = 1 << 16; // 64KB
        let memory = VectorMemory::new(WASM_PAGE_SIZE, None);
        assert_eq!(memory.size(), 65536);
    }

    #[test]
    fn test_memory_clone() {
        let mut original = VectorMemory::new(1024, None);
        original.set_i32(0, 0x12345678).unwrap();
        original.set_i32(100, -42).unwrap();

        let cloned = original.clone();

        // Both should have same size
        assert_eq!(original.size(), cloned.size());
        assert_eq!(cloned.size(), 1024);

        // Both should have same data
        assert_eq!(original.get_i32(0).unwrap(), cloned.get_i32(0).unwrap());
        assert_eq!(original.get_i32(100).unwrap(), cloned.get_i32(100).unwrap());
        assert_eq!(cloned.get_i32(0).unwrap(), 0x12345678);
        assert_eq!(cloned.get_i32(100).unwrap(), -42);
    }

    #[test]
    fn test_memory_clone_independence() {
        let mut original = VectorMemory::new(1024, None);
        original.set_i32(0, 100).unwrap();

        let mut cloned = original.clone();
        cloned.set_i32(0, 200).unwrap();

        // Changes to clone should not affect original
        assert_eq!(original.get_i32(0).unwrap(), 100);
        assert_eq!(cloned.get_i32(0).unwrap(), 200);
    }

    #[test]
    fn test_i32_operations() {
        let mut memory = VectorMemory::new(1024, None);

        // Test basic set/get
        memory.set_i32(0, 0x12345678).unwrap();
        assert_eq!(memory.get_i32(0).unwrap(), 0x12345678);

        // Test negative numbers
        memory.set_i32(4, -1).unwrap();
        assert_eq!(memory.get_i32(4).unwrap(), -1);

        // Test at different offsets
        memory.set_i32(100, 42).unwrap();
        assert_eq!(memory.get_i32(100).unwrap(), 42);

        // Original values should be unchanged
        assert_eq!(memory.get_i32(0).unwrap(), 0x12345678);
        assert_eq!(memory.get_i32(4).unwrap(), -1);
    }

    #[test]
    fn test_memory_bounds_checking() {
        let mut memory = VectorMemory::new(8, None);

        // Valid operations
        assert!(memory.set_i32(0, 123).is_ok());
        assert!(memory.set_i32(4, 456).is_ok());
        assert_eq!(memory.get_i32(0).unwrap(), 123);
        assert_eq!(memory.get_i32(4).unwrap(), 456);

        // Out of bounds operations
        assert!(memory.set_i32(5, 999).is_err()); // offset 5 + 4 bytes = 9 > size 8
        assert!(memory.get_i32(5).is_err());
        assert!(memory.set_i32(8, 999).is_err()); // exactly at boundary
        assert!(memory.get_i32(8).is_err());
    }

    #[test]
    fn test_little_endian_encoding() {
        let mut memory = VectorMemory::new(8, None);

        // Test that values are stored in little-endian format
        memory.set_i32(0, 0x12345678).unwrap();

        // Should be stored as [0x78, 0x56, 0x34, 0x12]
        assert_eq!(memory.get_u8(0).unwrap(), 0x78);
        assert_eq!(memory.get_u8(1).unwrap(), 0x56);
        assert_eq!(memory.get_u8(2).unwrap(), 0x34);
        assert_eq!(memory.get_u8(3).unwrap(), 0x12);
    }

    #[test]
    fn test_multiple_data_types() {
        let mut memory = VectorMemory::new(64, None);

        // Test various data types
        memory.set_u8(0, 0xFF).unwrap();
        memory.set_u16(1, 0x1234).unwrap();
        memory.set_i32(3, -42).unwrap();
        memory.set_i64(7, 0x123456789ABCDEF0_i64).unwrap();
        memory.set_f32(15, std::f32::consts::PI).unwrap();
        memory.set_f64(19, std::f64::consts::E).unwrap();

        assert_eq!(memory.get_u8(0).unwrap(), 0xFF);
        assert_eq!(memory.get_u16(1).unwrap(), 0x1234);
        assert_eq!(memory.get_i32(3).unwrap(), -42);
        assert_eq!(memory.get_i64(7).unwrap(), 0x123456789ABCDEF0_i64);
        assert!((memory.get_f32(15).unwrap() - std::f32::consts::PI).abs() < 0.00001);
        assert!((memory.get_f64(19).unwrap() - std::f64::consts::E).abs() < 0.000001);
    }

    #[test]
    fn test_memory_growth() {
        let mut memory = VectorMemory::new(1024, Some(2048));
        assert_eq!(memory.size(), 1024);

        // Test successful growth
        let new_size = memory.grow(1536).unwrap();
        assert_eq!(new_size, 1536);
        assert_eq!(memory.size(), 1536);

        // Test growth beyond maximum should fail
        assert!(memory.grow(3000).is_err());
    }

    #[test]
    fn test_memory_with_wasm_page_semantics() {
        const WASM_PAGE_SIZE: usize = 65536;

        // Test 1-page memory (typical for WASM modules)
        let mut memory = VectorMemory::new(WASM_PAGE_SIZE, None);
        assert_eq!(memory.size(), WASM_PAGE_SIZE);

        // Should be able to access anywhere in the page
        memory.set_i32(0, 1).unwrap();
        memory.set_i32(WASM_PAGE_SIZE - 4, 2).unwrap(); // Last 4 bytes

        assert_eq!(memory.get_i32(0).unwrap(), 1);
        assert_eq!(memory.get_i32(WASM_PAGE_SIZE - 4).unwrap(), 2);

        // Should not be able to access beyond the page
        assert!(memory.get_i32(WASM_PAGE_SIZE - 3).is_err()); // Would read beyond memory
        assert!(memory.set_i32(WASM_PAGE_SIZE, 3).is_err()); // Exactly at boundary
    }

    #[test]
    fn test_memory_corruption_detection() {
        let mut memory = VectorMemory::new(1024, None);

        // Fill memory with pattern
        for i in 0..256 {
            memory.set_i32(i * 4, i as i32).unwrap();
        }

        // Clone and verify both have same data
        let cloned = memory.clone();
        for i in 0..256 {
            assert_eq!(memory.get_i32(i * 4).unwrap(), i as i32);
            assert_eq!(cloned.get_i32(i * 4).unwrap(), i as i32);
        }

        // Modify original and verify clone is unchanged
        memory.set_i32(0, 999).unwrap();
        assert_eq!(memory.get_i32(0).unwrap(), 999);
        assert_eq!(cloned.get_i32(0).unwrap(), 0); // Should still be original value
    }

    /// Test that simulates the specific block_test scenario
    #[test]
    fn test_block_test_memory_scenario() {
        const WASM_PAGE_SIZE: usize = 65536;

        // Create memory like in block.wast: (memory 1)
        let mut memory = VectorMemory::new(WASM_PAGE_SIZE, None);
        assert_eq!(memory.size(), WASM_PAGE_SIZE);

        // The block_test tries to load i32 from address 1
        // Uninitialized memory should return 0
        assert_eq!(memory.get_i32(1).unwrap(), 0);

        // If we put value 1 at address 1, it should read back as 1
        memory.set_i32(1, 1).unwrap();
        assert_eq!(memory.get_i32(1).unwrap(), 1);

        // Clone the memory (like in the test execution)
        let cloned = memory.clone();
        assert_eq!(cloned.size(), WASM_PAGE_SIZE);
        assert_eq!(cloned.get_i32(1).unwrap(), 1);
    }
}
