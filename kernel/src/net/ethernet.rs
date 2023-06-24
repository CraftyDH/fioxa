use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::vec::Vec;
use kernel_userspace::{
    ids::ServiceID,
    net::Networking,
    service::{
        generate_tracking_number, get_public_service_id, SendServiceMessageDest, ServiceMessage,
        ServiceMessageType, ServiceTrackingNumber,
    },
    syscall::{
        receive_service_message_blocking, receive_service_message_blocking_tracking,
        send_and_get_response_service_message, send_service_message, service_create,
        service_subscribe, spawn_thread, yield_now,
    },
};
use modular_bitfield::{bitfield, specifiers::B48};
use x86_64::instructions::interrupts::without_interrupts;

use crate::{
    cpu_localstorage::get_task_mgr_current_pid,
    net::arp::{ARP, ARP_TABLE},
    service::PUBLIC_SERVICES,
    syscall::syssleep,
};

use super::arp::ARPEth;

// pub static RECEIVED_FRAMES_QUEUE: SegQueue<EthernetFrame> = SegQueue::new();

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
pub struct EthernetFrame<'a> {
    pub header: EthernetFrameHeader,
    pub data: &'a [u8],
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

const IP_ADDR: IPAddr = IPAddr::V4(10, 0, 2, 15);
const SUBNET: u32 = 0xFF0000;

pub fn send_arp(service: ServiceID, mac_addr: u64, ip: IPAddr) {
    if !IP_ADDR.same_subnet(&ip, SUBNET) {
        todo!("Not in same subnet: {:?}->{:?}", IP_ADDR, ip)
    }
    let mut arp = ARP::new();
    arp.set_hardware_type(1u16.to_be()); // Ethernet
    arp.set_protocol(0x0800u16.to_be()); // ipv4
    arp.set_hardware_addr_size(6); // mac
    arp.set_protocol_addr_size(4); // ipv4
    arp.set_operation(1u16.to_be()); // request

    arp.set_src_ip(IP_ADDR.as_net_be());
    arp.set_src_mac(mac_addr);
    arp.set_dst_ip(ip.as_net_be());

    let mut header = EthernetFrameHeader::new();
    header.set_dst_mac_be(0xFF_FF_FF_FF_FF_FF);
    header.set_src_mac_be(mac_addr);
    header.set_ether_type_be(0x0806u16.to_be());
    let arp_req = ARPEth { header, arp };
    let buf: &[u8; size_of::<ARPEth>()] = &unsafe { transmute(arp_req) };

    let mut buffer = Vec::new();

    let resp = send_and_get_response_service_message(
        &ServiceMessage {
            service_id: service,
            sender_pid: get_task_mgr_current_pid(),
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::PhysicalNet(
                kernel_userspace::net::PhysicalNet::SendPacket(buf),
            ),
        },
        &mut buffer,
    )
    .unwrap();
}

pub fn userspace_networking_main() {
    let sid = service_create();
    PUBLIC_SERVICES.lock().insert("NETWORKING", sid);

    let pcnet;
    let mut buffer = Vec::new();
    loop {
        if let Some(m) = get_public_service_id("PCNET", &mut buffer) {
            pcnet = m;
            service_subscribe(pcnet);
            break;
        }
        yield_now();
    }

    println!("SIDS: {sid:?} {pcnet:?}");

    let ServiceMessageType::PhysicalNet(kernel_userspace::net::PhysicalNet::MacAddrResp(mac)) = send_and_get_response_service_message(
        &kernel_userspace::service::ServiceMessage {
            service_id: pcnet,
            sender_pid: get_task_mgr_current_pid(),
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::PhysicalNet(
                kernel_userspace::net::PhysicalNet::MacAddrGet,
            ),
        },
        &mut Vec::new(),
    ).unwrap().message else {
        panic!()
    };

    spawn_thread(|| monitor_packets(pcnet));
    loop {
        let mut buf = Vec::new();
        let query = receive_service_message_blocking(sid, &mut buf).unwrap();
        let resp = match query.message {
            ServiceMessageType::Networking(net) => match net {
                Networking::ArpRequest(a, b, c, d) => {
                    let ip = IPAddr::V4(a, b, c, d);
                    let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

                    if let None = mac_addr {
                        send_arp(pcnet, mac, ip);
                    }

                    ServiceMessageType::Networking(Networking::ArpResponse(mac_addr))
                }
                _ => ServiceMessageType::UnknownCommand,
            },
            _ => ServiceMessageType::UnknownCommand,
        };

        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: get_task_mgr_current_pid(),
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
                message: resp,
            },
            &mut buf,
        )
        .unwrap();
    }
}

pub fn monitor_packets(pcnet: ServiceID) {
    loop {
        let mut buf = Vec::new();
        let message: ServiceMessage<'_> =
            receive_service_message_blocking_tracking(pcnet, ServiceTrackingNumber(0), &mut buf)
                .unwrap();
        match message.message {
            ServiceMessageType::PhysicalNet(
                kernel_userspace::net::PhysicalNet::ReceivedPacket(packet),
            ) => {
                let header = unsafe { *(packet.as_ptr() as *const EthernetFrameHeader) };
                let data = &packet[size_of::<EthernetFrameHeader>()..];

                handle_ethernet_frame(EthernetFrame { header, data })
            }
            _ => unimplemented!("{message:?}"),
        }
    }
}

pub fn lookup_ip(a: u8, b: u8, c: u8, d: u8) -> Option<u64> {
    let mut buf = Vec::new();
    let networking = get_public_service_id("NETWORKING", &mut buf).unwrap();
    for _ in 0..5 {
        match send_and_get_response_service_message(
            &kernel_userspace::service::ServiceMessage {
                service_id: networking,
                sender_pid: get_task_mgr_current_pid(),
                tracking_number: generate_tracking_number(),
                destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
                message: ServiceMessageType::Networking(
                    kernel_userspace::net::Networking::ArpRequest(a, b, c, d),
                ),
            },
            &mut buf,
        )
        .unwrap()
        .message
        {
            ServiceMessageType::Networking(Networking::ArpResponse(resp)) => {
                if let Some(_) = resp {
                    return resp;
                }
            }
            _ => unimplemented!(),
        }

        syssleep(1000)
    }
    None
}
