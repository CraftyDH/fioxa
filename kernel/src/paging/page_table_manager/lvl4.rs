use super::*;

impl PageTable<'_, PageLvl4> {
    fn idx(address: u64) -> usize {
        ((address >> (12 + 9 + 9 + 9)) & 0x1ff) as usize
    }

    pub fn get_lvl3(&mut self, address: u64) -> PageTable<'_, PageLvl3> {
        let table = self.table.get_or_create_table(Self::idx(address));
        PageTable {
            table,
            level: core::marker::PhantomData,
        }
    }

    pub unsafe fn set_lvl3_location(&mut self, address: u64, lvl3: &mut PageTable<'_, PageLvl3>) {
        assert!(address & 0x80000000 == 0 || address & 0x80000000 == 0x80000000);
        self.table.set_table(Self::idx(address), lvl3.table);
    }

    pub fn try_get_lvl3(&self, address: u64) -> Option<PageTable<'_, PageLvl3>> {
        let idx = (address >> (12 + 9 + 9 + 9)) & 0x1ff;
        let table = self.table.get_table(idx as usize)?;
        Some(PageTable {
            table,
            level: core::marker::PhantomData,
        })
    }

    pub fn get_lvl4_addr(&self) -> u64 {
        self.table as *const PhysPageTable as u64
    }

    pub unsafe fn shift_table_to_offset(&mut self) {
        self.table = &mut *((self.table as *mut PhysPageTable as *mut u8)
            .add(MemoryLoc::PhysMapOffset as usize)
            as *mut PhysPageTable)
    }

    // Lvl4. never has huge pages so just defer to lvl3 for all
    fn defer_map_memory<'l, S: PageSize>(
        &'l mut self,
        from: Page<S>,
        to: Page<S>,
    ) -> Option<Flusher>
    where
        PageTable<'l, PageLvl3>: Mapper<S>,
    {
        let mut lvl3 = self.get_lvl3(from.address);
        lvl3.map_memory(from, to)
    }
}

impl Mapper<Size1GB> for PageTable<'_, PageLvl4> {
    fn map_memory(&mut self, from: Page<Size1GB>, to: Page<Size1GB>) -> Option<Flusher> {
        self.defer_map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size1GB>) -> Option<Flusher> {
        self.get_lvl3(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size1GB>) -> Option<u64> {
        self.try_get_lvl3(page.address)?.get_phys_addr(page)
    }
}

impl Mapper<Size2MB> for PageTable<'_, PageLvl4> {
    fn map_memory(&mut self, from: Page<Size2MB>, to: Page<Size2MB>) -> Option<Flusher> {
        self.defer_map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size2MB>) -> Option<Flusher> {
        self.get_lvl3(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size2MB>) -> Option<u64> {
        self.try_get_lvl3(page.address)?.get_phys_addr(page)
    }
}

impl Mapper<Size4KB> for PageTable<'_, PageLvl4> {
    fn map_memory(&mut self, from: Page<Size4KB>, to: Page<Size4KB>) -> Option<Flusher> {
        self.defer_map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Option<Flusher> {
        self.get_lvl3(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size4KB>) -> Option<u64> {
        self.try_get_lvl3(page.address)?.get_phys_addr(page)
    }
}
