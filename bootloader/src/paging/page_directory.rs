use modular_bitfield::{
    bitfield,
    specifiers::{B3, B12, B40},
};

#[bitfield]
pub struct PageDirectoryEntry {
    pub present: bool,
    pub read_write: bool,
    pub user_super: bool,
    pub write_through: bool,
    pub cache_disabled: bool,
    pub accessed: bool,
    #[skip]
    _skip0: bool,
    pub larger_pages: bool,
    #[skip]
    _skip1: bool,
    pub available: B3,
    internal_address: B40,
    #[skip]
    _reserved: B12,
}

impl PageDirectoryEntry {
    // Shift address by 12 to fit the structure
    pub fn get_address(&self) -> u64 {
        self.internal_address() << 12
    }

    pub fn set_address(&mut self, address: u64) {
        self.set_internal_address(address >> 12)
    }
}

#[repr(C, align(0x1000))]
pub struct PageTable {
    pub entries: [PageDirectoryEntry; 512],
}

impl PageTable {
    pub fn has_entries(&mut self) -> bool {
        self.entries.iter().all(|pde| !pde.present())
    }
}
