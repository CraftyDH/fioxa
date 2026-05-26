use core::{
    fmt::Debug,
    mem::{size_of, transmute},
};

use alloc::{sync::Arc, vec::Vec};
use fioxa_rpc::{
    RPCHandleBuilder,
    client::RPCClient,
    net_capnp,
    server::RPCServer,
    service::{ServiceExecutor, get_and_connect_service},
};
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    mutex::Mutex,
    net::{IPAddr, NotSameSubnetError},
};
use modular_bitfield::{bitfield, specifiers::B48};

use crate::{
    net::arp::{ARP, ARP_TABLE, ARPEth},
    scheduling::with_held_interrupts,
};

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
    service: &mut RPCClient<fioxa_rpc::net_capnp::EthMessage>,
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

    let mut req = fioxa_rpc::net::SendPacket::new_req();
    req.init().set_packet(buf);
    service.send(&req.build()).unwrap();
    Ok(())
}

pub fn userspace_networking_main() {
    let eth = get_and_connect_service("ETHERNET").unwrap();
    let mut eth = RPCClient::new(eth);

    let mut req = fioxa_rpc::net::GetMac::new_req();
    req.init();
    let resp = eth.send(&req.build()).unwrap();
    let mac = resp.get_reply().unwrap().get_message().unwrap().get_val();

    let (listen_chan, listen_chan_right) = Channel::new();

    let mut req = fioxa_rpc::net::ListenToPackets::new_req();
    let mut handles = RPCHandleBuilder::new();
    handles.add(req.init().init_channel(), listen_chan_right.into_inner());

    eth.send(&req.build_handles(&handles)).unwrap();

    sys_process_spawn_thread(move || monitor_packets(listen_chan));

    let pcnet = Arc::new(Mutex::new(eth));

    ServiceExecutor::with_name("NETWORKING", |chan| {
        let network = pcnet.clone();

        sys_process_spawn_thread(move || RPCServer::new(chan, NetHandler { mac, network }).run());
    })
    .run()
    .unwrap();
}

struct NetHandler {
    mac: u64,
    network: Arc<Mutex<RPCClient<fioxa_rpc::net_capnp::EthMessage>>>,
}

impl fioxa_rpc::net::NetService for NetHandler {
    fn arp_request<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, net_capnp::arp_request::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, net_capnp::arp_reponse::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder,
    ) -> Result<(), ::capnp::Error> {
        let ip = IPAddr::ipv4_addr_from_net(req.get_ip());
        let mac_addr = ARP_TABLE.lock().get(&ip).cloned();

        match mac_addr {
            Some(mac) => {
                res.set_success(net_capnp::arp_reponse::ArpSuccess::Success);
                res.set_mac(mac);
            }
            None => match send_arp(&mut self.network.lock(), self.mac, ip) {
                Ok(()) => res.set_success(net_capnp::arp_reponse::ArpSuccess::Unknown),
                Err(NotSameSubnetError { .. }) => {
                    res.set_success(net_capnp::arp_reponse::ArpSuccess::NotSameSubnet)
                }
            },
        }
        Ok(())
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
