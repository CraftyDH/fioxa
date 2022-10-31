use alloc::collections::BTreeMap;
use modular_bitfield::{bitfield, specifiers::B48};
use spin::Mutex;

use super::ethernet::{EthernetFrameHeader, IPAddr};

#[bitfield]
pub struct ARP {
    pub hardware_type: u16,
    pub protocol: u16,
    pub hardware_addr_size: u8,
    pub protocol_addr_size: u8,
    pub operation: u16,

    pub src_mac: B48,
    pub src_ip: u32,
    pub dst_mac: B48,
    pub dst_ip: u32,
}

#[repr(C, packed)]
pub struct ARPEth {
    pub header: EthernetFrameHeader,
    pub arp: ARP,
}

lazy_static::lazy_static! {
    pub static ref ARP_TABLE: Mutex<BTreeMap<IPAddr, u64>> = Mutex::new(BTreeMap::new());
}
