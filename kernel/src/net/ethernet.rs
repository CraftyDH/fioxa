use core::{
    fmt::Debug,
    mem::{size_of, transmute},
    ops::ControlFlow,
};

use alloc::vec::Vec;
use kernel_userspace::{
    channel::{channel_create_rs, channel_read_rs, channel_write_rs},
    net::{ArpResponse, IPAddr, Networking, NotSameSubnetError},
    object::KernelReference,
    service::{deserialize, serialize, Service, SimpleService},
    syscall::spawn_thread,
};
use modular_bitfield::{bitfield, specifiers::B48};

use crate::{
    net::arp::{ARP, ARP_TABLE},
    scheduling::with_held_interrupts,
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
    trace!("{:?}", frame.header);
    if frame.header.ether_type_be() == 1544 {
        with_held_interrupts(|| {
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
    service: &mut SimpleService,
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

    let mut buffer = Vec::new();
    serialize(
        &kernel_userspace::net::PhysicalNet::SendPacket(buf),
        &mut buffer,
    );

    let mut handles = Vec::new();
    service.call(&mut buffer, &mut handles).unwrap();

    Ok(())
}

pub fn userspace_networking_main() {
    let mut pcnet = SimpleService::with_name("PCNET");

    let mut buffer = Vec::with_capacity(100);

    serialize(&kernel_userspace::net::PhysicalNet::MacAddrGet, &mut buffer);
    pcnet.call(&mut buffer, &mut Vec::new()).unwrap();
    let mac: u64 = deserialize(&buffer).unwrap();

    let (listen_chan, listen_chan_right) = channel_create_rs();

    serialize(
        &kernel_userspace::net::PhysicalNet::ListenToPackets,
        &mut buffer,
    );
    let mut handles = Vec::new();
    handles.push(listen_chan_right.id());
    pcnet.call(&mut buffer, &mut handles).unwrap();

    spawn_thread(move || monitor_packets(listen_chan));

    Service::new(
        "NETWORKING",
        || (),
        |handle, ()| {
            match channel_read_rs(handle.id(), &mut buffer, &mut Vec::new()) {
                kernel_userspace::channel::ChannelReadResult::Ok => (),
                kernel_userspace::channel::ChannelReadResult::Closed => {
                    return ControlFlow::Break(())
                }
                e => {
                    warn!("{e:?}");
                    return ControlFlow::Break(());
                }
            }

            match deserialize(&buffer) {
                Ok(Networking::ArpRequest(ip)) => {
                    let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

                    let resp = match mac_addr {
                        Some(mac) => ArpResponse::Mac(mac),
                        None => ArpResponse::Pending(send_arp(&mut pcnet, mac, ip)),
                    };

                    serialize(&resp, &mut buffer);
                    channel_write_rs(handle.id(), &buffer, &[]);
                }
                Err(e) => {
                    warn!("Bad message: {e:?}");
                    return ControlFlow::Break(());
                }
            };

            ControlFlow::Continue(())
        },
    )
    .run();
}

pub fn monitor_packets(socket: KernelReference) {
    let mut buffer = Vec::with_capacity(2048);
    loop {
        match channel_read_rs(socket.id(), &mut buffer, &mut Vec::new()) {
            kernel_userspace::channel::ChannelReadResult::Ok => (),
            e => panic!("{e:?}"),
        };

        assert!(buffer.len() > size_of::<EthernetFrameHeader>());

        let header = unsafe { *(buffer.as_ptr() as *const EthernetFrameHeader) };
        let data = &buffer[size_of::<EthernetFrameHeader>()..];

        handle_ethernet_frame(EthernetFrame { header, data })
    }
}
