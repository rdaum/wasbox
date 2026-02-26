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

use crate::exec::Fault;
use crate::Memory;
use std::cell::UnsafeCell;
use std::rc::Rc;

/// Growable memory backed by a vector.
/// VectorMemory is Clone so that the memory can be fully copied when a new execution on a pre-linked
/// module is started.
/// TODO: Reality is we could probably use some sort of Copy-on-Write strategy here, but for now we just
///   clone the memory.
pub struct VectorMemory {
    max_bounds: Option<usize>,
    data: Rc<UnsafeCell<Vec<u8>>>,
}

impl Clone for VectorMemory {
    fn clone(&self) -> Self {
        VectorMemory {
            max_bounds: self.max_bounds,
            data: self.data.clone(),
        }
    }
}

impl VectorMemory {
    pub fn new(min_size: usize, max_bounds: Option<usize>) -> Self {
        VectorMemory {
            max_bounds,
            data: Rc::new(UnsafeCell::new(vec![0; min_size])),
        }
    }

    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        // Shared backing storage is required for imported memories.
        unsafe { &mut *self.data.get() }
    }
}

impl Memory for VectorMemory {
    fn data(&self) -> &[u8] {
        // Shared backing storage is required for imported memories.
        unsafe { &*self.data.get() }
    }

    fn data_mut(&mut self) -> &mut [u8] {
        // Shared backing storage is required for imported memories.
        unsafe { &mut *self.data.get() }
    }

    fn size(&self) -> usize {
        // Shared backing storage is required for imported memories.
        unsafe { (&*self.data.get()).len() }
    }
    fn grow(&mut self, new_size: usize) -> Result<usize, Fault> {
        if let Some(max) = self.max_bounds {
            if new_size > max {
                return Err(Fault::CannotGrowMemory);
            }
        }

        // Prevent excessive memory allocations (1GB limit for safety)
        if new_size > 1024 * 1024 * 1024 {
            return Err(Fault::CannotGrowMemory);
        }

        // Shared backing storage is required for imported memories.
        let data = unsafe { &mut *self.data.get() };
        data.resize(new_size, 0);
        Ok(data.len())
    }
}
