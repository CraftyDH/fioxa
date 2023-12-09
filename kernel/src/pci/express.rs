use alloc::{boxed::Box, sync::Arc};

use crate::paging::{
    page_table_manager::{ensure_ident_map_curr_process, Page, Size4KB},
    MemoryMappingFlags,
};

use super::{mcfg::MCFG, PCIBus, PCIDevice, PCIHeaderCommon};

pub struct ExpressPCI<'mcfg> {
    mcfg: &'mcfg MCFG,
}

pub struct PCIExpressDevice {
    address: u64,
}

impl PCIDevice for PCIExpressDevice {
    unsafe fn read_u8(&self, offset: u32) -> u8 {
        *((self.address + offset as u64) as *const u8)
    }

    unsafe fn read_u16(&self, offset: u32) -> u16 {
        assert!(offset % 2 == 0);
        *((self.address + offset as u64) as *const u16)
    }

    unsafe fn read_u32(&self, offset: u32) -> u32 {
        assert!(offset % 4 == 0);
        *((self.address + offset as u64) as *const u32)
    }

    unsafe fn write_u8(&mut self, offset: u32, data: u8) {
        *((self.address + offset as u64) as *mut u8) = data;
    }

    unsafe fn write_u16(&mut self, offset: u32, data: u16) {
        assert!(offset % 2 == 0);
        *((self.address + offset as u64) as *mut u16) = data;
    }

    unsafe fn write_u32(&mut self, offset: u32, data: u32) {
        assert!(offset % 4 == 0);
        *((self.address + offset as u64) as *mut u32) = data;
    }
}

impl<'mcfg> ExpressPCI<'mcfg> {
    pub fn new(mcfg: &'mcfg MCFG) -> Self {
        Self { mcfg }
    }

    fn get_address(&self, segment: u16, bus: u8, device: u8, function: u8) -> Option<u64> {
        let offset = (bus as u64) << 20 | (device as u64) << 15 | (function as u64) << 12;

        for entry in self.mcfg.entries() {
            if entry.pci_segment_group == segment {
                return Some(entry.base_address + offset);
            }
        }
        None
    }
}

impl<'mcfg> PCIBus for ExpressPCI<'mcfg> {
    fn get_device(&mut self, segment: u16, bus: u8, device: u8, function: u8) -> PCIHeaderCommon {
        let address = self.get_address(segment, bus, device, function).unwrap();

        unsafe {
            ensure_ident_map_curr_process(
                Page::<Size4KB>::new(address),
                MemoryMappingFlags::WRITEABLE,
            )
        };

        PCIHeaderCommon {
            device: Arc::new(PCIExpressDevice { address }),
        }
    }
    fn get_device_raw(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDevice> {
        let address = self.get_address(segment, bus, device, function).unwrap();

        unsafe {
            ensure_ident_map_curr_process(
                Page::<Size4KB>::new(address),
                MemoryMappingFlags::WRITEABLE,
            )
        };

        Box::new(PCIExpressDevice { address })
    }
}
