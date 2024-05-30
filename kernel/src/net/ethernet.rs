use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::{sync::Arc, vec::Vec};
use kernel_userspace::{
    backoff_sleep,
    message::MessageHandle,
    net::{ArpResponse, IPAddr, Networking, NotSameSubnetError},
    object::KernelObjectType,
    service::{deserialize, make_message_new},
    socket::{SocketHandle, SocketListenHandle},
    syscall::spawn_thread,
};
use modular_bitfield::{bitfield, specifiers::B48};
use x86_64::instructions::interrupts::without_interrupts;

use crate::net::arp::{ARP, ARP_TABLE};

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
    trace!("{:?}", frame.header);
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

pub fn send_arp(
    service: &SocketHandle,
    mac_addr: u64,
    ip: IPAddr,
) -> Result<(), NotSameSubnetError> {
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

    let msg = make_message_new(&kernel_userspace::net::PhysicalNet::SendPacket(buf));
    service.blocking_send(msg.kref()).unwrap();
    // wait for ack
    service.blocking_recv().unwrap();

    Ok(())
}

pub fn userspace_networking_main() {
    let service = SocketListenHandle::listen("NETWORKING").unwrap();

    let pcnet = backoff_sleep(|| SocketHandle::connect("PCNET"));

    let msg = make_message_new(&kernel_userspace::net::PhysicalNet::MacAddrGet);
    pcnet.blocking_send(msg.kref()).unwrap();
    let (mac, ty) = pcnet.blocking_recv().unwrap();

    assert_eq!(ty, KernelObjectType::Message);
    let mac = MessageHandle::from_kref(mac).read_vec();
    let mac: u64 = deserialize(&mac).unwrap();

    let msg = make_message_new(&kernel_userspace::net::PhysicalNet::ListenToPackets);
    pcnet.blocking_send(msg.kref()).unwrap();
    let (listen, ty) = pcnet.blocking_recv().unwrap();

    assert_eq!(ty, KernelObjectType::Socket);
    let listener = SocketHandle::from_raw_socket(listen);

    spawn_thread(move || monitor_packets(listener));
    let pcnet = Arc::new(pcnet);
    loop {
        let query = service.blocking_accept();
        let (q, ty) = query.blocking_recv().unwrap();

        if ty != KernelObjectType::Message {
            error!("usernetworking invalid message");
            continue;
        }

        let q = MessageHandle::from_kref(q).read_vec();
        match deserialize(&q) {
            Ok(Networking::ArpRequest(ip)) => {
                let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

                let resp = match mac_addr {
                    Some(mac) => ArpResponse::Mac(mac),
                    None => ArpResponse::Pending(send_arp(&pcnet, mac, ip)),
                };

                let resp = make_message_new(&resp);
                if query.blocking_send(resp.kref()).is_err() {
                    info!("usernetworking eof");
                    continue;
                }
            }
            Err(_) => continue,
        };
    }
}

pub fn monitor_packets(socket: SocketHandle) {
    let mut buffer = Vec::new();
    loop {
        let (message, ty) = socket.blocking_recv().unwrap();

        assert_eq!(ty, KernelObjectType::Message);
        MessageHandle::from_kref(message).read_into_vec(&mut buffer);

        assert!(buffer.len() > size_of::<EthernetFrameHeader>());

        let header = unsafe { *(buffer.as_ptr() as *const EthernetFrameHeader) };
        let data = &buffer[size_of::<EthernetFrameHeader>()..];

        handle_ethernet_frame(EthernetFrame { header, data })
    }
}
