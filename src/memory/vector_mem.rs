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

use crate::exec::Fault;
use crate::Memory;

/// Growable memory backed by a vector.
/// VectorMemory is Clone so that the memory can be fully copied when a new execution on a pre-linked
/// module is started.
/// TODO: Reality is we could probably use some sort of Copy-on-Write strategy here, but for now we just
///   clone the memory.
pub struct VectorMemory {
    max_bounds: Option<usize>,
    data: Vec<u8>,
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
            data: vec![0; min_size],
        }
    }

    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }
}

impl Memory for VectorMemory {
    fn data(&self) -> &[u8] {
        &self.data
    }

    fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    fn size(&self) -> usize {
        self.data.len()
    }
    fn grow(&mut self, new_size: usize) -> Result<usize, Fault> {
        if let Some(max) = self.max_bounds {
            if new_size > max {
                return Err(Fault::CannotGrowMemory);
            }
        }
        self.data.resize(new_size, 0);
        Ok(self.data.len())
    }
}
