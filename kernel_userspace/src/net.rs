use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PhysicalNet<'a> {
    MacAddrGet,
    MacAddrResp(u64),
    SendPacket(&'a [u8]),
    ReceivedPacket(&'a [u8]),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Networking {
    ArpRequest(u8, u8, u8, u8),
    ArpResponse(Option<u64>),
}
