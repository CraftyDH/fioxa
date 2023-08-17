#![allow(dead_code)]
use modular_bitfield::{bitfield, specifiers::B5};

#[bitfield]
#[derive(Debug)]
pub(super) struct CapabilitiesIDRegister {
    pub rev_id: u8,
    pub timer_cnt: B5,
    pub can_64_bit: bool,
    #[skip]
    _resv: bool,
    pub legacy_replacement_cap: bool,
    pub vendor_id: u16,
    pub counter_tick_period: u32,
}
