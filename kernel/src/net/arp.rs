use alloc::collections::BTreeMap;
use conquer_once::spin::Lazy;
use modular_bitfield::{bitfield, specifiers::B48};

use spin::mutex::Mutex;

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

pub static ARP_TABLE: Lazy<Mutex<BTreeMap<IPAddr, u64>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));
