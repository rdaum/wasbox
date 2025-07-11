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

/// Non-growable generic backed by an un-owned slice.
pub struct SliceMemory<'a> {
    data: &'a mut [u8],
}

impl<'a> SliceMemory<'a> {
    pub fn new(data: &'a mut [u8]) -> Self {
        SliceMemory { data }
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        self.data
    }
}

impl Memory for SliceMemory<'_> {
    fn data(&self) -> &[u8] {
        self.data
    }

    fn data_mut(&mut self) -> &mut [u8] {
        self.data
    }

    fn size(&self) -> usize {
        self.data.len()
    }
    fn grow(&mut self, _new_size: usize) -> Result<usize, Fault> {
        Err(Fault::CannotGrowMemory)
    }
}
