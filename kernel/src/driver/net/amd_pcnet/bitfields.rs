#![allow(dead_code)]
use modular_bitfield::{
    bitfield,
    specifiers::{B4, B48},
};

#[bitfield]
pub(super) struct InitBlock {
    pub mode: u16,
    #[skip]
    _resv: B4,
    pub num_send_buffers: B4,
    #[skip]
    _resv2: B4,
    pub num_recv_buffers: B4,
    pub physical_address: B48,
    #[skip]
    pub _resv3: u16,
    pub logical_address: u64,
    pub recv_buffer_desc_addr: u32,
    pub send_buffer_desc_addr: u32,
}
