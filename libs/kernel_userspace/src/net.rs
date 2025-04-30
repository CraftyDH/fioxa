use core::fmt::Display;

use kernel_sys::types::SyscallResult;
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::InlineAsBox,
};
use thiserror::Error;

use crate::{
    channel::Channel,
    ipc::{CowAsOwned, IPCChannel},
};

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum PhysicalNet<'a> {
    MacAddrGet,
    SendPacket(Slice<'a>),
    ListenToPackets(CowAsOwned<'a, Channel>),
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct Slice<'a>(#[rkyv(with = InlineAsBox)] pub &'a [u8]);

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum Networking {
    ArpRequest(IPAddr),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Archive, Serialize, Deserialize)]
pub enum IPAddr {
    V4(u8, u8, u8, u8),
}

impl Display for IPAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IPAddr::V4(a, b, c, d) => f.write_fmt(format_args!("IPV4({a}.{b}.{c}.{d})")),
        }
    }
}

#[derive(Debug, Clone, Error, Archive, Serialize, Deserialize)]
#[error("ips not in same subnet a: `{a}`, b: `{b}` with subnet of `{subnet:#X}`")]
pub struct NotSameSubnetError {
    a: IPAddr,
    b: IPAddr,
    subnet: u32,
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

    pub fn same_subnet(&self, ip2: &IPAddr, subnet: u32) -> Result<(), NotSameSubnetError> {
        match (self, ip2) {
            (Self::V4(a1, b1, c1, d1), Self::V4(a2, b2, c2, d2)) => {
                let ip1_u32 =
                    (*a1 as u32) << 24 | (*b1 as u32) << 16 | (*c1 as u32) << 8 | *d1 as u32;
                let ip2_u32 =
                    (*a2 as u32) << 24 | (*b2 as u32) << 16 | (*c2 as u32) << 8 | *d2 as u32;
                if (ip1_u32 & subnet) == (ip2_u32 & subnet) {
                    Ok(())
                } else {
                    Err(NotSameSubnetError {
                        a: self.clone(),
                        b: ip2.clone(),
                        subnet,
                    })
                }
            }
        }
    }
}

pub struct NetworkInterfaceService(IPCChannel);

impl NetworkInterfaceService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self(chan)
    }

    pub fn mac_address(&mut self) -> u64 {
        self.0.send(&PhysicalNet::MacAddrGet).assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }

    pub fn send_packet(&mut self, packet: &[u8]) {
        self.0
            .send(&PhysicalNet::SendPacket(Slice(packet)))
            .assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }

    pub fn listen_to_packets(&mut self, chan: Channel) {
        self.0
            .send(&PhysicalNet::ListenToPackets(CowAsOwned(
                alloc::borrow::Cow::Owned(chan),
            )))
            .assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }
}

pub struct NetworkInterfaceServiceExecutor<I: NetworkInterfaceServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: NetworkInterfaceServiceImpl> NetworkInterfaceServiceExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedPhysicalNet>()?;

            let err = match msg {
                ArchivedPhysicalNet::MacAddrGet => self.channel.send(&self.service.mac_address()),
                ArchivedPhysicalNet::SendPacket(packet) => {
                    self.service.send_packet(&packet.0);
                    self.channel.send(&())
                }
                ArchivedPhysicalNet::ListenToPackets(channel) => {
                    self.service.listen_to_packets(channel.0.deserialize(des)?);
                    self.channel.send(&())
                }
            };
            err.into_err().map_err(Error::new)?;
        }
    }
}

pub trait NetworkInterfaceServiceImpl {
    fn mac_address(&mut self) -> u64;

    fn send_packet(&mut self, packet: &[u8]);

    fn listen_to_packets(&mut self, channel: Channel);
}

pub struct NetService(IPCChannel);

impl NetService {
    pub fn from_channel(channel: IPCChannel) -> Self {
        Self(channel)
    }

    pub fn arp_request(&mut self, ip: IPAddr) -> Result<Option<u64>, NotSameSubnetError> {
        self.0.send(&Networking::ArpRequest(ip)).assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }
}

pub trait NetServiceImpl {
    fn arp_request(&mut self, ip: IPAddr) -> Result<Option<u64>, NotSameSubnetError>;
}

pub struct NetServiceExecutor<I: NetServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: NetServiceImpl> NetServiceExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedNetworking>()?;

            let err = match msg {
                ArchivedNetworking::ArpRequest(ip) => {
                    let res = self.service.arp_request(ip.deserialize(des)?);
                    self.channel.send(&res)
                }
            };
            err.into_err().map_err(Error::new)?;
        }
    }
}
