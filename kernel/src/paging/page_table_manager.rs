use super::{
    get_uefi_active_mapper,
    page_allocator::{free_page, request_page},
    page_directory::{PageDirectoryEntry, PageTable},
    page_map_index::PageMapIndexer,
};

pub struct PageTableManager {
    page_lvl4_addr: u64,
}

pub fn ident_map_curr_process(memory: u64, write: bool) {
    let mut mapper = unsafe { get_uefi_active_mapper() };
    mapper.map_memory(memory, memory, write).unwrap().flush();
}

impl PageTableManager {
    pub fn map_memory(
        &mut self,
        virtual_memory: u64,
        physical_memory: u64,
        write: bool,
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
            pde.set_read_write(write);
            Ok(Flusher(virtual_memory))
        })
    }

    pub fn unmap_memory(&self, virtual_memory: u64) -> Result<Flusher, &str> {
        let indexer = PageMapIndexer::new(virtual_memory);
        let pml4 = unsafe { &mut *(self.page_lvl4_addr as *mut PageTable) };

        let pdp = Self::get_table(&mut pml4.entries[indexer.pdp_i as usize])
            .ok_or("Couldn't read pdp")?;

        let pd =
            Self::get_table(&mut pdp.entries[indexer.pd_i as usize]).ok_or("Couldn't read pd")?;

        let pt =
            Self::get_table(&mut pd.entries[indexer.pt_i as usize]).ok_or("Couldn't read pt")?;

        let pde = &mut pt.entries[indexer.p_i as usize];
        pde.set_present(false);
        pde.set_address(0);

        // Free mapping pages if possible
        if pt.has_entries() {
            let entry = &mut pd.entries[indexer.pt_i as usize];
            free_page(entry.get_address());
            entry.set_present(false);
            entry.set_address(0);
        }

        if pd.has_entries() {
            let entry = &mut pdp.entries[indexer.pd_i as usize];
            free_page(entry.get_address());
            entry.set_present(false);
            entry.set_address(0);
        }

        if pdp.has_entries() {
            let entry = &mut pml4.entries[indexer.pdp_i as usize];
            free_page(entry.get_address());
            entry.set_present(false);
            entry.set_address(0);
        }
        Ok(Flusher(virtual_memory))
    }

    pub fn get_phys_addr(&self, virtual_memory: u64) -> Result<u64, &str> {
        let indexer = PageMapIndexer::new(virtual_memory);
        let pml4 = unsafe { &mut *(self.page_lvl4_addr as *mut PageTable) };

        let pdp = Self::get_table(&mut pml4.entries[indexer.pdp_i as usize])
            .ok_or("Couldn't read pdp")?;

        let pd =
            Self::get_table(&mut pdp.entries[indexer.pd_i as usize]).ok_or("Couldn't read pd")?;

        let pt =
            Self::get_table(&mut pd.entries[indexer.pt_i as usize]).ok_or("Couldn't read pt")?;

        let pde = &mut pt.entries[indexer.p_i as usize];
        Ok(pde.get_address())
    }
}

impl PageTableManager {
    pub const fn new(page_lvl4_addr: u64) -> Self {
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
