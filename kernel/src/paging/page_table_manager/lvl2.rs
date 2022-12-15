use super::*;

impl PageTable<'_, PageLvl2> {
    fn idx(address: u64) -> usize {
        ((address >> (12 + 9)) & 0x1ff) as usize
    }
    pub fn get_lvl1(&mut self, address: u64) -> PageTable<'_, PageLvl1> {
        let table = self.table.get_or_create_table(Self::idx(address));
        PageTable {
            table,
            level: core::marker::PhantomData,
        }
    }

    pub fn try_get_lvl1(&self, address: u64) -> Option<PageTable<'_, PageLvl1>> {
        let table = self.table.get_table(Self::idx(address))?;
        Some(PageTable {
            table,
            level: core::marker::PhantomData,
        })
    }
}

impl Mapper<Size2MB> for PageTable<'_, PageLvl2> {
    fn map_memory(&mut self, from: Page<Size2MB>, to: Page<Size2MB>) -> Option<Flusher> {
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

    fn unmap_memory(&mut self, page: Page<Size2MB>) -> Option<Flusher> {
        let entry = &mut self.table.entries[Self::idx(page.address)];

        if !entry.present() {
            println!("WARN: attempting to unmap something that is not mapped");
            return None;
        }

        entry.set_present(false);
        entry.set_address(0);

        Some(Flusher(page.address))
    }

    fn get_phys_addr(&self, page: Page<Size2MB>) -> Option<u64> {
        let idx = (page.address >> (12 + 9)) & 0x1ff;
        let entry = &self.table.entries[idx as usize];

        if !entry.present() {
            return None;
        }

        if entry.larger_pages() {
            Some(entry.get_address() | idx << (12 + 9))
        } else {
            // TODO: Better error
            None
        }
    }
}

impl Mapper<Size4KB> for PageTable<'_, PageLvl2> {
    fn map_memory(&mut self, from: Page<Size4KB>, to: Page<Size4KB>) -> Option<Flusher> {
        self.get_lvl1(from.address).map_memory(from, to)
    }
    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Option<Flusher> {
        self.get_lvl1(page.address).unmap_memory(page)
    }

    fn get_phys_addr(&self, page: Page<Size4KB>) -> Option<u64> {
        let idx = (page.address >> (12 + 9)) & 0x1ff;
        let entry = &self.table.entries[idx as usize];

        if !entry.present() {
            return None;
        }

        if entry.larger_pages() {
            Some(entry.get_address() | idx << (12 + 9))
        } else {
            let table =
                unsafe { &mut *(virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable) };
            let lvl1: PageTable<PageLvl1> = PageTable {
                table,
                level: core::marker::PhantomData,
            };
            lvl1.get_phys_addr(page)
        }
    }
}
