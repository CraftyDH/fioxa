use core::fmt::Display;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PhysicalNet<'a> {
    MacAddrGet,
    SendPacket(&'a [u8]),
    ListenToPackets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Networking {
    ArpRequest(IPAddr),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkingResp {
    ArpResponse(ArpResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArpResponse {
    Mac(u64),
    Pending(Result<(), NotSameSubnetError>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
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
