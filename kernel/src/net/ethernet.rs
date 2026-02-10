use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::{sync::Arc, vec::Vec};
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    ipc::IPCChannel,
    mutex::Mutex,
    net::{
        IPAddr, NetServiceExecutor, NetServiceImpl, NetworkInterfaceService, NotSameSubnetError,
    },
    service::ServiceExecutor,
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
    service: &mut NetworkInterfaceService,
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
    let buf = unsafe { &transmute::<ARPEth, [u8; size_of::<ARPEth>()]>(arp_req) };

    service.send_packet(buf);

    Ok(())
}

pub fn userspace_networking_main() {
    let mut pcnet = NetworkInterfaceService::from_channel(IPCChannel::connect("PCNET"));

    let mac = pcnet.mac_address();

    let (listen_chan, listen_chan_right) = Channel::new();

    pcnet.listen_to_packets(listen_chan_right);

    sys_process_spawn_thread(move || monitor_packets(listen_chan));

    let pcnet = Arc::new(Mutex::new(pcnet));

    ServiceExecutor::with_name("NETWORKING", |chan| {
        let network = pcnet.clone();

        sys_process_spawn_thread(move || {
            match NetServiceExecutor::new(
                IPCChannel::from_channel(chan),
                NetHandler { mac, network },
            )
            .run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
}

struct NetHandler {
    mac: u64,
    network: Arc<Mutex<NetworkInterfaceService>>,
}

impl NetServiceImpl for NetHandler {
    fn arp_request(&mut self, ip: IPAddr) -> Result<Option<u64>, NotSameSubnetError> {
        let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

        match mac_addr {
            Some(mac) => Ok(Some(mac)),
            None => send_arp(&mut self.network.lock(), self.mac, ip).map(|()| None),
        }
    }
}

pub fn monitor_packets(channel: Channel) {
    let mut buffer = Vec::new();
    loop {
        channel.read::<0>(&mut buffer, true, true).unwrap();

        assert!(buffer.len() > size_of::<EthernetFrameHeader>());

        let header = unsafe { *(buffer.as_ptr() as *const EthernetFrameHeader) };
        let data = &buffer[size_of::<EthernetFrameHeader>()..];

        handle_ethernet_frame(EthernetFrame { header, data })
    }
}
