use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::{sync::Arc, vec::Vec};
use crossbeam_queue::SegQueue;
use kernel_userspace::syscall::yield_now;
use lazy_static::lazy_static;
use modular_bitfield::{bitfield, specifiers::B48};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::{
    driver::net::{EthernetDriver, SendError},
    net::arp::{ARP, ARP_TABLE},
    syscall::syssleep,
};

use super::arp::ARPEth;

lazy_static! {
    pub static ref RECEIVED_FRAMES_QUEUE: SegQueue<EthernetFrame> = SegQueue::new();
}

#[bitfield]
#[derive(Clone, Copy)]
pub struct EthernetFrameHeader {
    pub dst_mac_be: B48,
    pub src_mac_be: B48,
    pub ether_type_be: u16,
}

impl Debug for EthernetFrameHeader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EthernetFrameHeader")
            .field("dst_MAC", &format_args!("{:X}", self.dst_mac_be()))
            .field("src_MAC", &format_args!("{:X}", self.src_mac_be()))
            .field("ether_type", &self.ether_type_be())
            .finish()
    }
}

#[derive(Debug)]
pub struct EthernetFrame {
    pub header: EthernetFrameHeader,
    pub data: Vec<u8>,
}

pub fn ethernet_task() {
    loop {
        while let Some(frame) = RECEIVED_FRAMES_QUEUE.pop() {
            handle_ethernet_frame(frame);
        }
        yield_now();
    }
}

pub fn handle_ethernet_frame(frame: EthernetFrame) {
    println!("{:?}", frame.header);
    if frame.header.ether_type_be() == 1544 {
        without_interrupts(|| {
            assert!(frame.data.len() >= size_of::<ARP>());
            let arp = unsafe { &*(frame.data.as_ptr() as *const ARP) };
            if arp.src_mac() != 0xFF_FF_FF && arp.src_mac() != 0 {
                ARP_TABLE
                    .lock()
                    .insert(IPAddr::ipv4_addr_from_net(arp.src_ip()), arp.src_mac());
            }
            if arp.dst_mac() != 0xFF_FF_FF && arp.dst_mac() != 0 {
                ARP_TABLE
                    .lock()
                    .insert(IPAddr::ipv4_addr_from_net(arp.dst_ip()), arp.dst_mac());
            }
        });
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IPAddr {
    V4(u8, u8, u8, u8),
}

impl IPAddr {
    pub fn ipv4_addr_from_net(ip: u32) -> Self {
        Self::V4(
            ip as u8,
            (ip >> 8) as u8,
            (ip >> 16) as u8,
            (ip >> 24) as u8,
        )
    }

    pub fn as_net_be(&self) -> u32 {
        match self {
            Self::V4(a, b, c, d) => {
                *a as u32 | (*b as u32) << 8 | (*c as u32) << 16 | (*d as u32) << 24
            }
        }
    }

    pub fn same_subnet(&self, ip2: &IPAddr, subnet: u32) -> bool {
        match (self, ip2) {
            (Self::V4(a1, b1, c1, d1), Self::V4(a2, b2, c2, d2)) => {
                let ip1 = (*a1 as u32) << 24 | (*b1 as u32) << 16 | (*c1 as u32) << 8 | *d1 as u32;
                let ip2 = (*a2 as u32) << 24 | (*b2 as u32) << 16 | (*c2 as u32) << 8 | *d2 as u32;
                return (ip1 & subnet) == (ip2 & subnet);
            }
        }
    }
}

lazy_static! {
    pub static ref ETHERNET: Mutex<Ethernet> = Mutex::new(Ethernet {
        devices: Vec::new()
    });
}

pub struct EthernetDevice {
    driver: Arc<Mutex<dyn EthernetDriver>>,
    ip_addr: IPAddr,
    mac_addr: u64,
    subnet: u32,
}

pub struct Ethernet {
    devices: Vec<EthernetDevice>,
}

impl Ethernet {
    pub fn new_device(&mut self, driver: Arc<Mutex<dyn EthernetDriver>>) {
        let mac_addr = driver.lock().read_mac_addr();
        self.devices.push(EthernetDevice {
            driver,
            ip_addr: IPAddr::V4(192, 168, 1, 100),
            mac_addr,
            subnet: 0xFF0000,
        })
    }

    pub fn send_arp(&mut self, ip: IPAddr) {
        for device in self.devices.iter_mut() {
            if !device.ip_addr.same_subnet(&ip, device.subnet) {
                todo!("Not in same subnet: {:?}->{:?}", device.ip_addr, ip)
            }
            let mut arp = ARP::new();
            arp.set_hardware_type(1u16.to_be()); // Ethernet
            arp.set_protocol(0x0800u16.to_be()); // ipv4
            arp.set_hardware_addr_size(6); // mac
            arp.set_protocol_addr_size(4); // ipv4
            arp.set_operation(1u16.to_be()); // request

            arp.set_src_ip(device.ip_addr.as_net_be());
            arp.set_src_mac(device.mac_addr);
            arp.set_dst_ip(ip.as_net_be());

            let mut header = EthernetFrameHeader::new();
            header.set_dst_mac_be(0xFF_FF_FF_FF_FF_FF);
            header.set_src_mac_be(device.mac_addr);
            header.set_ether_type_be(0x0806u16.to_be());
            let arp_req = ARPEth { header, arp };
            let buf: &[u8; size_of::<ARPEth>()] = &unsafe { transmute(arp_req) };
            while let Err(SendError::BufferFull) = without_interrupts(|| {
                device
                    .driver
                    .try_lock()
                    .ok_or(SendError::BufferFull)
                    .and_then(|mut d| d.send_packet(buf))
            }) {
                yield_now()
            }
            return;
        }
    }
}

pub fn lookup_ip(ip: IPAddr) -> Option<u64> {
    for _ in 0..5 {
        if let Some(mac) = without_interrupts(|| ARP_TABLE.lock().get(&ip).cloned()) {
            return Some(mac);
        };
        ETHERNET.lock().send_arp(ip.clone());
        syssleep(1000)
    }
    None
}
