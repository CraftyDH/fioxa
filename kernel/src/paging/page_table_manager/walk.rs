use super::*;

impl<S: PageSize> Mapper<S> for PageTable<'_, PageLvl4>
where
    for<'a> PageTable<'a, PageLvl3>: Mapper<S>,
{
    fn map_memory(&mut self, from: Page<S>, to: Page<S>) -> Result<Flusher, MapMemoryError> {
        self.get_next_table(from).map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<S>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_walk_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<S>) -> Option<u64> {
        self.get_next_table(page).get_phys_addr(page)
    }
}

impl<S: PageSize> Mapper<S> for PageTable<'_, PageLvl3>
where
    for<'a> PageTable<'a, PageLvl2>: Mapper<S>,
{
    fn map_memory(&mut self, from: Page<S>, to: Page<S>) -> Result<Flusher, MapMemoryError> {
        self.get_next_table(from).map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<S>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_walk_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<S>) -> Option<u64> {
        self.get_next_table(page).get_phys_addr(page)
    }
}

impl<S: PageSize> Mapper<S> for PageTable<'_, PageLvl2>
where
    for<'a> PageTable<'a, PageLvl1>: Mapper<S>,
{
    fn map_memory(&mut self, from: Page<S>, to: Page<S>) -> Result<Flusher, MapMemoryError> {
        self.get_next_table(from).map_memory(from, to)
    }

    fn unmap_memory(&mut self, page: Page<S>) -> Result<Flusher, UnMapMemoryError> {
        self.unmap_memory_walk_inner(page)
    }

    fn get_phys_addr(&mut self, page: Page<S>) -> Option<u64> {
        self.get_next_table(page).get_phys_addr(page)
    }
}
