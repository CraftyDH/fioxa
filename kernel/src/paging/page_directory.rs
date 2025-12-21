use modular_bitfield::{
    bitfield,
    specifiers::{B3, B11, B40},
};

#[bitfield(bits = 64)]
pub struct PageDirectoryEntry {
    pub present: bool,
    pub read_write: bool,
    pub user_super: bool,
    pub write_through: bool,
    pub cache_disabled: bool,
    pub accessed: bool,
    pub dirty: bool,
    pub larger_pages: bool,
    pub global: bool,
    pub available: B3,
    internal_address: B40,
    pub fx_owned: bool,
    #[skip]
    _reserved: B11,
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
