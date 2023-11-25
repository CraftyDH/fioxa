use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::vec::Vec;
use kernel_userspace::{
    ids::ServiceID,
    net::{ArpResponse, IPAddr, Networking, NetworkingResp, NotSameSubnetError},
    service::{
        generate_tracking_number, get_public_service_id, register_public_service,
        SendServiceMessageDest, ServiceMessage, ServiceTrackingNumber,
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
    cpu_localstorage::CPULocalStorageRW,
    net::arp::{ARP, ARP_TABLE},
};

use super::arp::ARPEth;

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

const IP_ADDR: IPAddr = IPAddr::V4(10, 0, 2, 15);
const SUBNET: u32 = 0xFF0000;

pub fn send_arp(service: ServiceID, mac_addr: u64, ip: IPAddr) -> Result<(), NotSameSubnetError> {
    IP_ADDR.same_subnet(&ip, SUBNET)?;
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

    send_and_get_response_service_message::<_, kernel_userspace::net::PhysicalNetResp>(
        &ServiceMessage {
            service_id: service,
            sender_pid: CPULocalStorageRW::get_current_pid(),
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            message: kernel_userspace::net::PhysicalNet::SendPacket(buf),
        },
        &mut buffer,
    )
    .unwrap();
    Ok(())
}

pub fn userspace_networking_main() {
    let sid = service_create();
    register_public_service("NETWORKING", sid, &mut Vec::new());

    let mut buffer = Vec::new();
    let pcnet = loop {
        if let Some(m) = get_public_service_id("PCNET", &mut buffer) {
            break m;
        }
        yield_now();
    };

    service_subscribe(pcnet);

    println!("SIDS: {sid:?} {pcnet:?}");

    let kernel_userspace::net::PhysicalNetResp::MacAddrResp(mac) =
        send_and_get_response_service_message(
            &kernel_userspace::service::ServiceMessage {
                service_id: pcnet,
                sender_pid: CPULocalStorageRW::get_current_pid(),
                tracking_number: generate_tracking_number(),
                destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
                message: kernel_userspace::net::PhysicalNet::MacAddrGet,
            },
            &mut Vec::new(),
        )
        .unwrap()
        .message
    else {
        panic!()
    };

    spawn_thread(move || monitor_packets(pcnet));
    loop {
        let mut buf = Vec::new();
        let query = receive_service_message_blocking(sid, &mut buf).unwrap();
        let resp = match query.message {
            Networking::ArpRequest(ip) => {
                let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

                let resp = match mac_addr {
                    Some(mac) => ArpResponse::Mac(mac),
                    None => ArpResponse::Pending(send_arp(pcnet, mac, ip)),
                };

                NetworkingResp::ArpResponse(resp)
            }
        };

        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: CPULocalStorageRW::get_current_pid(),
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
        let message =
            receive_service_message_blocking_tracking(pcnet, ServiceTrackingNumber(0), &mut buf)
                .unwrap();
        match message.message {
            kernel_userspace::net::PhysicalNetResp::ReceivedPacket(packet) => {
                let header = unsafe { *(packet.as_ptr() as *const EthernetFrameHeader) };
                let data = &packet[size_of::<EthernetFrameHeader>()..];

                handle_ethernet_frame(EthernetFrame { header, data })
            }
            _ => unimplemented!("{message:?}"),
        }
    }
}
