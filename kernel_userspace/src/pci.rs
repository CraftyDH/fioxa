use alloc::sync::Arc;
use serde::{Deserialize, Serialize};
use spin::Mutex;

use crate::channel::Channel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PCIDevCmd {
    Read(u32),
    Write(u32, u32),
}

pub struct PCIDevice {
    pub device_service: Channel,
}

#[allow(dead_code)]
impl PCIDevice {
    unsafe fn read_u8(&mut self, offset: u32) -> u8 {
        unsafe {
            let block = self.read_u32(offset & !0b11);
            ((block >> 8 * (offset & 0b11)) & 0xFF) as u8
        }
    }

    unsafe fn read_u16(&mut self, offset: u32) -> u16 {
        unsafe {
            let block = self.read_u32(offset & !0b11);
            ((block >> 8 * (offset & 0b11)) & 0xFFFF) as u16
        }
    }

    unsafe fn read_u32(&mut self, offset: u32) -> u32 {
        self.device_service
            .call_val::<0, _, _>(&PCIDevCmd::Read(offset), &[])
            .unwrap()
            .0
    }

    unsafe fn write_u8(&mut self, offset: u32, data: u8) {
        unsafe {
            let mut block = self.read_u32(offset & !0b11);
            block &= !(0xFF << 8 * (offset & 0b11));
            block |= (data as u32) << 8 * (offset & 0b11);
            self.write_u32(offset & !0b11, block);
        }
    }

    unsafe fn write_u16(&mut self, offset: u32, data: u16) {
        unsafe {
            let mut block = self.read_u32(offset & !0b10);
            block &= !(0xFFFF << 8 * (offset & 0b11));
            block |= (data as u32) << 8 * (offset & 0b11);
            self.write_u32(offset & !0b11, block);
        }
    }

    unsafe fn write_u32(&mut self, offset: u32, data: u32) {
        self.device_service
            .call_val::<0, _, _>(&PCIDevCmd::Write(offset, data), &[])
            .unwrap()
            .0
    }
}

pub struct PCIHeaderCommon {
    pub device: Arc<Mutex<PCIDevice>>,
}

impl PCIHeaderCommon {
    pub fn get_vendor_id(&self) -> u16 {
        unsafe { self.device.lock().read_u16(0) }
    }
    pub fn get_device_id(&self) -> u16 {
        unsafe { self.device.lock().read_u16(2) }
    }

    pub fn get_command(&self) -> u16 {
        unsafe { self.device.lock().read_u16(4) }
    }

    pub fn get_status(&self) -> u16 {
        unsafe { self.device.lock().read_u16(6) }
    }

    pub fn get_revision_id(&self) -> u8 {
        unsafe { self.device.lock().read_u8(8) }
    }

    pub fn get_prog_if(&self) -> u8 {
        unsafe { self.device.lock().read_u8(9) }
    }

    pub fn set_prog_if(&self) -> u8 {
        unsafe { self.device.lock().read_u8(9) }
    }

    pub fn get_subclass(&self) -> u8 {
        unsafe { self.device.lock().read_u8(10) }
    }

    pub fn get_class(&self) -> u8 {
        unsafe { self.device.lock().read_u8(11) }
    }

    pub fn get_cache_line_size(&self) -> u8 {
        unsafe { self.device.lock().read_u8(12) }
    }

    pub fn get_latency_timer(&self) -> u8 {
        unsafe { self.device.lock().read_u8(13) }
    }

    pub fn get_header_type(&self) -> u8 {
        unsafe { self.device.lock().read_u8(14) }
    }

    pub fn get_bist(&self) -> u8 {
        unsafe { self.device.lock().read_u8(15) }
    }

    pub unsafe fn get_as_header0(self) -> PCIHeader0 {
        PCIHeader0 {
            device: self.device.clone(),
        }
    }
}

pub struct PCIHeader0 {
    device: Arc<Mutex<PCIDevice>>,
}

impl PCIHeader0 {
    pub fn common(&self) -> PCIHeaderCommon {
        PCIHeaderCommon {
            device: self.device.clone(),
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
        unsafe { self.device.lock().read_u32(0x10 + bar_num as u32 * 4) }
    }

    pub fn get_interrupt_num(&self) -> u8 {
        unsafe { self.device.lock().read_u8(0x3C) }
    }
}
