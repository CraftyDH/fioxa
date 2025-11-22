use uefi::boot::{AllocateType, MemoryType, allocate_pages};

use super::{
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
            // !FIX: Kernel load sections has overlapping R/RW sections :(
            pde.set_read_write(pde.read_write() | write);
            if pde.present() {
                return Err("already mapped");
            }
            pde.set_present(true);
            pde.set_address(physical_memory);
            Ok(Flusher(virtual_memory))
        })
    }

    pub fn protect_memory(&mut self, virtual_memory: u64) -> Result<Flusher, &str> {
        let indexer = PageMapIndexer::new(virtual_memory);
        let pml4 = unsafe { &mut *(self.page_lvl4_addr as *mut PageTable) };

        let pdp = Self::get_or_create_table(&mut pml4.entries[indexer.pdp_i as usize]);

        let pd =
            pdp.and_then(|pdp| Self::get_or_create_table(&mut pdp.entries[indexer.pd_i as usize]));

        let pt =
            pd.and_then(|pd| Self::get_or_create_table(&mut pd.entries[indexer.pt_i as usize]));

        let pte = pt.ok_or("Could not traverse pml4 tree")?;
        let pde = &mut pte.entries[indexer.p_i as usize];
        if !pde.present() {
            return Err("not mapped");
        }
        pde.set_read_write(false);
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

    fn get_or_create_table(pde: &mut PageDirectoryEntry) -> Option<&'static mut PageTable> {
        if pde.present() {
            return unsafe { Some(&mut *(pde.get_address() as *mut PageTable)) };
        }
        let new_page = allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
            .unwrap()
            .as_ptr();
        unsafe {
            core::ptr::write_bytes(new_page, 0, 0x1000);
        }
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
