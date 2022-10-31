pub mod amd_pcnet;

use alloc::sync::Arc;

use crate::{driver::driver::Driver, net::ethernet::EthernetFrame};

pub trait EthernetDriver: Driver {
    // Send a packet
    fn send_packet(&mut self, data: &[u8]) -> Result<(), SendError>;
    // Where should the device send it's received packets
    fn register_receive_buffer(&mut self, buffer: Arc<crossbeam_queue::SegQueue<EthernetFrame>>);

    fn read_mac_addr(&mut self) -> u64;
}

pub enum SendError {
    BufferFull,
}
