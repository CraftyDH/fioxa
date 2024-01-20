use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{
    ids::ServiceID,
    service::{generate_tracking_number, make_message_new, ServiceMessageDesc},
    syscall::{get_pid, send_and_get_response_service_message},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PCIDevCmd {
    Ack,
    UnknownCommand,
    Read(u32),
    Data(u32),
    Write(u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub struct PCIDevice {
    pub device_service: ServiceID,
}

#[allow(dead_code)]
impl PCIDevice {
    unsafe fn read_u8(&self, offset: u32) -> u8 {
        let block = self.read_u32(offset & !0b11);
        ((block >> 8 * (offset & 0b11)) & 0xFF) as u8
    }

    unsafe fn read_u16(&self, offset: u32) -> u16 {
        let block = self.read_u32(offset & !0b11);
        ((block >> 8 * (offset & 0b11)) & 0xFFFF) as u16
    }

    unsafe fn read_u32(&self, offset: u32) -> u32 {
        let PCIDevCmd::Data(data) = send_and_get_response_service_message(
            &ServiceMessageDesc {
                service_id: self.device_service,
                sender_pid: get_pid(),
                tracking_number: generate_tracking_number(),
                destination: crate::service::SendServiceMessageDest::ToProvider,
            },
            &make_message_new(&PCIDevCmd::Read(offset)),
        )
        .read(&mut Vec::new())
        .unwrap() else {
            todo!()
        };

        data
    }

    unsafe fn write_u8(&mut self, offset: u32, data: u8) {
        let mut block = self.read_u32(offset & !0b11);
        block &= !(0xFF << 8 * (offset & 0b11));
        block |= (data as u32) << 8 * (offset & 0b11);
        self.write_u32(offset & !0b11, block);
    }

    unsafe fn write_u16(&mut self, offset: u32, data: u16) {
        let mut block = self.read_u32(offset & !0b10);
        block &= !(0xFFFF << 8 * (offset & 0b11));
        block |= (data as u32) << 8 * (offset & 0b11);
        self.write_u32(offset & !0b11, block);
    }

    unsafe fn write_u32(&mut self, offset: u32, data: u32) {
        let PCIDevCmd::Ack = send_and_get_response_service_message(
            &ServiceMessageDesc {
                service_id: self.device_service,
                sender_pid: get_pid(),
                tracking_number: generate_tracking_number(),
                destination: crate::service::SendServiceMessageDest::ToProvider,
            },
            &make_message_new(&PCIDevCmd::Write(offset, data)),
        )
        .read(&mut Vec::new())
        .unwrap() else {
            todo!()
        };
    }
}

pub struct PCIHeaderCommon {
    pub device: PCIDevice,
}

impl PCIHeaderCommon {
    pub fn get_vendor_id(&self) -> u16 {
        unsafe { self.device.read_u16(0) }
    }
    pub fn get_device_id(&self) -> u16 {
        unsafe { self.device.read_u16(2) }
    }

    pub fn get_command(&self) -> u16 {
        unsafe { self.device.read_u16(4) }
    }

    pub fn get_status(&self) -> u16 {
        unsafe { self.device.read_u16(6) }
    }

    pub fn get_revision_id(&self) -> u8 {
        unsafe { self.device.read_u8(8) }
    }

    pub fn get_prog_if(&self) -> u8 {
        unsafe { self.device.read_u8(9) }
    }

    pub fn set_prog_if(&self) -> u8 {
        unsafe { self.device.read_u8(9) }
    }

    pub fn get_subclass(&self) -> u8 {
        unsafe { self.device.read_u8(10) }
    }

    pub fn get_class(&self) -> u8 {
        unsafe { self.device.read_u8(11) }
    }

    pub fn get_cache_line_size(&self) -> u8 {
        unsafe { self.device.read_u8(12) }
    }

    pub fn get_latency_timer(&self) -> u8 {
        unsafe { self.device.read_u8(13) }
    }

    pub fn get_header_type(&self) -> u8 {
        unsafe { self.device.read_u8(14) }
    }

    pub fn get_bist(&self) -> u8 {
        unsafe { self.device.read_u8(15) }
    }

    pub unsafe fn get_as_header0(self) -> PCIHeader0 {
        PCIHeader0 {
            device: self.device,
        }
    }
}

pub struct PCIHeader0 {
    device: PCIDevice,
}

impl PCIHeader0 {
    pub fn common(&self) -> PCIHeaderCommon {
        PCIHeaderCommon {
            device: self.device,
        }
    }

    pub fn get_port_base(&self) -> Option<u32> {
        for i in 0..5 {
            let bar = self.get_bar(i);
            let address = bar & 0xFFFFFFFC;
            if address > 0 && bar & 1 == 1 {
                return Some(address);
            }
        }
        None
    }

    pub fn get_bar(&self, bar_num: u8) -> u32 {
        assert!(bar_num <= 5);
        unsafe { self.device.read_u32(0x10 + bar_num as u32 * 4) }
    }

    pub fn get_interrupt_num(&self) -> u8 {
        unsafe { self.device.read_u8(0x3C) }
    }
}
