#![allow(dead_code)]
use modular_bitfield::{
    bitfield,
    specifiers::{B4, B5, B9, B22, B128},
};

#[bitfield]
pub(super) struct HBACommandHeader {
    pub command_fis_length: B5,
    pub atapi: bool,
    pub write: bool,
    pub prefetchable: bool,

    pub reset: bool,
    pub bist: bool,
    pub clear_busy: bool,
    #[skip]
    _rsv0: bool,
    pub port_multiplier: B4,

    pub prdt_length: u16,
    pub prdb_count: u32,
    // Don't think I have to have upper and lower as seperate u32's
    pub command_table_base_address: u64,
    #[skip]
    _rev1: B128,
}

#[bitfield]
pub struct HBAPRDTEntry {
    //* Supposed to be lower + upper u32's
    pub data_base_address: u64,
    #[skip]
    _rsv0: u32,
    pub byte_count: B22,
    #[skip]
    _rsv1: B9,
    pub interrupt_on_completion: bool,
}
