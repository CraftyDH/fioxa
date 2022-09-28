use alloc::boxed::Box;

use crate::paging::get_uefi_active_mapper;

use super::{mcfg::MCFG, PCIBus, PCIDeviceCommonHeader};

pub struct ExpressPCI<'mcfg> {
    mcfg: &'mcfg MCFG,
}

#[repr(C, packed)]
pub struct PCIDeviceCommonHeaderExpressInternal {
    vendor_id: u16,
    device_id: u16,
    command: u16,
    status: u16,
    revision_id: u8,
    prog_if: u8,
    subclass: u8,
    class: u8,
    cache_line_size: u8,
    latency_timer: u8,
    header_type: u8,
    bist: u8,
}
pub struct PCIDeviceCommonHeaderExpress {
    internal: *mut PCIDeviceCommonHeaderExpressInternal,
}

impl PCIDeviceCommonHeaderExpress {
    fn get_internal(&mut self) -> &mut PCIDeviceCommonHeaderExpressInternal {
        unsafe { &mut *self.internal }
    }
}

impl<'a> PCIDeviceCommonHeader for PCIDeviceCommonHeaderExpress {
    fn get_vendor_id(&mut self) -> u16 {
        self.get_internal().vendor_id
    }

    fn get_device_id(&mut self) -> u16 {
        self.get_internal().device_id
    }

    fn get_command(&mut self) -> u16 {
        self.get_internal().command
    }

    fn get_status(&mut self) -> u16 {
        self.get_internal().status
    }

    fn get_revision_id(&mut self) -> u8 {
        self.get_internal().revision_id
    }

    fn get_prog_if(&mut self) -> u8 {
        self.get_internal().prog_if
    }

    fn get_subclass(&mut self) -> u8 {
        self.get_internal().subclass
    }

    fn get_class(&mut self) -> u8 {
        self.get_internal().class
    }

    fn get_cache_line_size(&mut self) -> u8 {
        self.get_internal().cache_line_size
    }

    fn get_latency_timer(&mut self) -> u8 {
        self.get_internal().latency_timer
    }

    fn get_header_type(&mut self) -> u8 {
        self.get_internal().header_type
    }

    fn get_bist(&mut self) -> u8 {
        self.get_internal().bist
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
    fn get_device(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDeviceCommonHeader> {
        let addr = self.get_address(segment, bus, device, function).unwrap()
            as *mut PCIDeviceCommonHeaderExpressInternal;

        let mut mapper = unsafe { get_uefi_active_mapper() };

        mapper.map_memory(addr as u64, addr as u64).unwrap().flush();

        Box::new(PCIDeviceCommonHeaderExpress { internal: addr })
    }
}
