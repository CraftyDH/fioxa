use core::ops::Add;

use crate::paging::MemoryLoc;

use super::*;

impl PageTable<'_, PageLvl4> {
    pub unsafe fn shift_table_to_offset(&mut self) {
        self.table = &mut *((self.table as *mut PhysPageTable)
            .addr()
            .add(MemoryLoc::PhysMapOffset as usize)
            as *mut PhysPageTable);
    }

    pub unsafe fn load_into_cr3(&self) {
        let cr3 = self.into_page().get_address();
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) cr3,
            options(nostack, preserves_flags)
        );
    }

    pub unsafe fn load_into_cr3_lazy(&self) {
        let cr3 = self.into_page().get_address();
        let current_cr3: u64;
        core::arch::asm!(
            "mov {}, cr3",
            lateout(reg) current_cr3,
            options(nostack, preserves_flags)
        );
        if cr3 != current_cr3 {
            core::arch::asm!(
                "mov cr3, {}",
                in(reg) cr3,
                options(nostack, preserves_flags)
            );
        }
    }

    pub fn get_phys_addr_from_vaddr(&mut self, address: u64) -> Option<u64> {
        Some(self.get_phys_addr(Page::<Size4KB>::containing(address))? + address % 0x1000)
    }
}

impl Mapper<Size1GB> for PageTable<'_, PageLvl3> {
    fn map_memory(
        &mut self,
        from: Page<Size1GB>,
        to: Page<Size1GB>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        self.map_memory_inner(from, to, flags)
    }

    fn unmap_memory(&mut self, page: Page<Size1GB>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size1GB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}

impl Mapper<Size2MB> for PageTable<'_, PageLvl2> {
    fn map_memory(
        &mut self,
        from: Page<Size2MB>,
        to: Page<Size2MB>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        self.map_memory_inner(from, to, flags)
    }

    fn unmap_memory(&mut self, page: Page<Size2MB>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size2MB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}

impl Mapper<Size4KB> for PageTable<'_, PageLvl1> {
    fn map_memory(
        &mut self,
        from: Page<Size4KB>,
        to: Page<Size4KB>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        self.map_memory_inner(from, to, flags)
    }

    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<Size4KB>) -> Option<u64> {
        self.get_phys_addr_inner(page)
    }
}
