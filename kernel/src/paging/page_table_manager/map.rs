use crate::paging::MemoryLoc;

use super::*;

impl PageTable<'_, PageLvl4> {
    pub fn get_lvl4_addr(&self) -> u64 {
        self.table as *const PhysPageTable as u64
    }

    pub unsafe fn shift_table_to_offset(&mut self) {
        self.table = &mut *((self.table as *mut PhysPageTable as *mut u8)
            .add(MemoryLoc::PhysMapOffset as usize)
            as *mut PhysPageTable)
    }
}

impl Mapper<Size1GB> for PageTable<'_, PageLvl3> {
    fn map_memory(&mut self, from: Page<Size1GB>, to: Page<Size1GB>) -> Option<Flusher> {
        self.map_memory_inner(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size1GB>) -> Option<Flusher> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size1GB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}

impl Mapper<Size2MB> for PageTable<'_, PageLvl2> {
    fn map_memory(&mut self, from: Page<Size2MB>, to: Page<Size2MB>) -> Option<Flusher> {
        self.map_memory_inner(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size2MB>) -> Option<Flusher> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size2MB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}

impl Mapper<Size4KB> for PageTable<'_, PageLvl1> {
    fn map_memory(&mut self, from: Page<Size4KB>, to: Page<Size4KB>) -> Option<Flusher> {
        self.map_memory_inner(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Option<Flusher> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size4KB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}
