use super::*;

impl PageTable<'_, PageLvl1> {}

impl Mapper<Size4KB> for PageTable<'_, PageLvl1> {
    fn map_memory(&mut self, from: Page<Size4KB>, to: Page<Size4KB>) -> Option<Flusher> {
        let idx = (from.address >> (12)) & 0x1ff;

        let entry = &mut self.table.entries[idx as usize];
        entry.set_present(true);
        entry.set_address(to.address);
        entry.set_read_write(true);
        entry.set_user_super(true);

        Some(Flusher(from.address))
    }

    fn unmap_memory(&mut self, page: Page<Size4KB>) -> Option<Flusher> {
        let idx = (page.address >> (12)) & 0x1ff;

        let entry = &mut self.table.entries[idx as usize];
        entry.set_present(false);
        entry.set_address(0);

        Some(Flusher(page.address))
    }

    fn get_phys_addr(&self, page: Page<Size4KB>) -> Option<u64> {
        let idx = (page.address >> (12)) & 0x1ff;

        let entry = &self.table.entries[idx as usize];
        if entry.present() {
            Some(entry.get_address())
        } else {
            None
        }
    }
}
