use super::*;

impl PageTable<'_, PageLvl3> {
    fn idx(address: u64) -> usize {
        ((address >> (12 + 9 + 9)) & 0x1ff) as usize
    }

    pub fn get_lvl2(&mut self, address: u64) -> PageTable<'_, PageLvl2> {
        let idx = (address >> (12 + 9 + 9)) & 0x1ff;
        let table = self.table.get_or_create_table(idx as usize);
        PageTable {
            table,
            level: core::marker::PhantomData,
        }
    }

    pub fn try_get_lvl2(&self, address: u64) -> Option<PageTable<'_, PageLvl2>> {
        let idx = (address >> (12 + 9 + 9)) & 0x1ff;
        let table = self.table.get_table(idx as usize)?;
        Some(PageTable {
            table,
            level: core::marker::PhantomData,
        })
    }

    // Lvl4. never has huge pages so just defer to lvl3 for all
    fn defer_map_memory<'l, S: PageSize>(
        &'l mut self,
        from: Page<S>,
        to: Page<S>,
    ) -> Option<Flusher>
    where
        PageTable<'l, PageLvl2>: Mapper<S>,
    {
        let mut lvl2 = self.get_lvl2(from.address);
        lvl2.map_memory(from, to)
    }

    fn defer_get_phys_addr<'l, S: PageSize>(&self, page: Page<S>) -> Option<u64>
    where
        PageTable<'l, PageLvl2>: Mapper<S>,
    {
        let idx = (page.address >> (12 + 9 + 9)) & 0x1ff;
        let entry = &self.table.entries[idx as usize];

        if !entry.present() {
            return None;
        }

        if entry.larger_pages() {
            Some(entry.get_address() | idx << (12 + 9 + 9))
        } else {
            let table =
                unsafe { &mut *(virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable) };
            let lvl2: PageTable<PageLvl2> = PageTable {
                table,
                level: core::marker::PhantomData,
            };
            lvl2.get_phys_addr(page)
        }
    }
}

impl Mapper<Size4KB> for PageTable<'_, PageLvl3> {
    fn map_memory(&mut self, from: Page<Size4KB>, to: Page<Size4KB>) -> Option<Flusher> {
        self.defer_map_memory(from, to)
    }
    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Option<Flusher> {
        self.get_lvl2(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size4KB>) -> Option<u64> {
        self.defer_get_phys_addr(page)
    }
}

impl Mapper<Size2MB> for PageTable<'_, PageLvl3> {
    fn map_memory(&mut self, from: Page<Size2MB>, to: Page<Size2MB>) -> Option<Flusher> {
        self.defer_map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<Size2MB>) -> Option<Flusher> {
        self.get_lvl2(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size2MB>) -> Option<u64> {
        self.defer_get_phys_addr(page)
    }
}

impl Mapper<Size1GB> for PageTable<'_, PageLvl3> {
    fn map_memory(&mut self, from: Page<Size1GB>, to: Page<Size1GB>) -> Option<Flusher> {
        let entry = &mut self.table.entries[Self::idx(from.address)];

        if entry.present() {
            println!("WARN: overiding mapping")
        }

        entry.set_present(true);
        entry.set_larger_pages(true);
        entry.set_address(to.address);
        entry.set_read_write(true);
        entry.set_user_super(true);

        Some(Flusher(from.address))
    }

    fn unmap_memory(&mut self, page: Page<Size1GB>) -> Option<Flusher> {
        let entry = &mut self.table.entries[Self::idx(page.address)];

        if !entry.present() {
            println!("WARN: attempting to unmap something that is not mapped");
            return None;
        }

        entry.set_present(false);
        entry.set_address(0);

        Some(Flusher(page.address))
    }

    fn get_phys_addr(&self, page: Page<Size1GB>) -> Option<u64> {
        let idx = (page.address >> (12 + 9 + 9)) & 0x1ff;
        let entry = &self.table.entries[idx as usize];

        if !entry.present() {
            return None;
        }

        if entry.larger_pages() {
            Some(entry.get_address() | idx << (12 + 9 + 9))
        } else {
            // TODO: Better error
            None
        }
    }
}
