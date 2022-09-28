use super::{
    page_allocator::request_page,
    page_directory::{PageDirectoryEntry, PageTable},
    page_map_index::PageMapIndexer,
};

pub struct PageTableManager {
    page_lvl4_addr: u64,
}

impl PageTableManager {
    pub fn map_memory(
        &mut self,
        virtual_memory: u64,
        physical_memory: u64,
    ) -> Result<Flusher, &str> {
        let indexer = PageMapIndexer::new(virtual_memory);
        let pml4 = unsafe { &mut *(self.page_lvl4_addr as *mut PageTable) };

        let pdp = Self::get_or_create_table(&mut pml4.entries[indexer.pdp_i as usize]);

        let pd =
            pdp.and_then(|pdp| Self::get_or_create_table(&mut pdp.entries[indexer.pd_i as usize]));

        let pt =
            pd.and_then(|pd| Self::get_or_create_table(&mut pd.entries[indexer.pt_i as usize]));

        pt.ok_or("Could not traverse pml4 tree").and_then(|pt| {
            let pde = &mut pt.entries[indexer.p_i as usize];
            pde.set_present(true);
            pde.set_address(physical_memory);
            pde.set_read_write(true);
            Ok(Flusher(virtual_memory))
        })
    }

    pub fn unmap_memory(&self, virtual_memory: u64) -> Result<Flusher, &str> {
        let indexer = PageMapIndexer::new(virtual_memory);
        let pml4 = unsafe { &mut *(self.page_lvl4_addr as *mut PageTable) };

        let pdp = Self::get_table(&mut pml4.entries[indexer.pdp_i as usize]);

        let pd = pdp.and_then(|pdp| Self::get_table(&mut pdp.entries[indexer.pd_i as usize]));

        let pt = pd.and_then(|pd| Self::get_table(&mut pd.entries[indexer.pt_i as usize]));

        pt.ok_or("Could not traverse pml4 tree").and_then(|pt| {
            let pde = &mut pt.entries[indexer.p_i as usize];
            pde.set_present(false);
            pde.set_address(0);
            Ok(Flusher(virtual_memory))
        })
        // TODO: Free pages
    }
}

impl PageTableManager {
    pub fn new(page_lvl4_addr: u64) -> Self {
        Self { page_lvl4_addr }
    }

    pub fn get_lvl4_addr(&self) -> u64 {
        self.page_lvl4_addr
    }

    fn get_table(pde: &mut PageDirectoryEntry) -> Option<&mut PageTable> {
        if pde.present() {
            return Some(unsafe { &mut *(pde.get_address() as *mut PageTable) });
        }
        None
    }

    fn get_or_create_table(pde: &mut PageDirectoryEntry) -> Option<&mut PageTable> {
        if pde.present() {
            return unsafe { Some(&mut *(pde.get_address() as *mut PageTable)) };
        }
        let new_page = request_page()?;
        let pdp = unsafe { &mut *(new_page as *mut PageTable) };
        pde.set_address(new_page as u64);
        pde.set_present(true);
        pde.set_read_write(true);
        Some(pdp)
    }

    fn check_del_table(pde: &mut PageTable) -> bool {
        pde.entries.iter().all(|pde| !pde.present())
    }

    pub fn load_into_cr3(&self) {
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) self.page_lvl4_addr, options(nostack, preserves_flags))
        };
    }
}

#[must_use = "TLB must be flushed or can be ignored"]
pub struct Flusher(u64);

impl Flusher {
    pub fn flush(self) {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) self.0, options(nostack, preserves_flags))
        }
    }

    pub fn ignore(self) {}
}
