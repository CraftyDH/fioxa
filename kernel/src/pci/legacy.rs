use alloc::{boxed::Box, sync::Arc};
use x86_64::instructions::port::Port;

use crate::mutex::Spinlock;

use super::{PCIBus, PCIDevice, PCIHeaderCommon};

pub static LEGACY_PCI_COMMAND: Spinlock<LegacyPCICommand> = Spinlock::new(LegacyPCICommand {
    data_port: Port::new(0xCFC),
    command_port: Port::new(0xCF8),
});

pub struct LegacyPCICommand {
    data_port: Port<u32>,
    command_port: Port<u32>,
}

impl LegacyPCICommand {
    unsafe fn read(&mut self, address: u32) -> u32 {
        // Send the device ID to the PCI controller
        self.command_port.write(address);

        // Read the result
        self.data_port.read()
    }

    unsafe fn write(&mut self, address: u32, value: u32) {
        // Send the device ID to the PCI controller
        self.command_port.write(address);

        // Read the result
        self.data_port.write(value)
    }
}

pub struct LegacyPCI {}

impl LegacyPCI {
    /// Returnes the base address of the given bus device and function
    fn get_address(bus: u8, device: u8, function: u8) -> u32 {
        // Get the 32 bit address
        let id = (1 << 31) // Enable bit
            | (bus as u32) << 16
            | (device as u32) << 11
            | (function as u32) << 8;
        id
    }
}

impl PCIBus for LegacyPCI {
    fn get_device(&mut self, segment: u16, bus: u8, device: u8, function: u8) -> PCIHeaderCommon {
        assert!(segment == 0);
        let new_header = PCILegacyDevice::new(bus, device, function);
        PCIHeaderCommon {
            device: Arc::new(new_header),
        }
    }

    fn get_device_raw(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDevice> {
        assert!(segment == 0);
        let new_header = PCILegacyDevice::new(bus, device, function);
        Box::new(new_header)
    }
}

/// Refer to the ubove struct PCIDeviceCommonHeaderExpress
/// for the ordering of the blocks
pub struct PCILegacyDevice {
    base_address: u32,
}

impl PCILegacyDevice {
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            base_address: LegacyPCI::get_address(bus, device, function),
        }
    }
}

impl PCIDevice for PCILegacyDevice {
    unsafe fn read_u8(&self, offset: u32) -> u8 {
        let block = self.read_u32(offset & !0b11);
        ((block >> 8 * (offset & 0b11)) & 0xFF) as u8
    }

    unsafe fn read_u16(&self, offset: u32) -> u16 {
        let block = self.read_u32(offset & !0b11);
        ((block >> 8 * (offset & 0b11)) & 0xFFFF) as u16
    }

    unsafe fn read_u32(&self, offset: u32) -> u32 {
        LEGACY_PCI_COMMAND.lock().read(self.base_address + offset)
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
        LEGACY_PCI_COMMAND
            .lock()
            .write(self.base_address + offset, data);
    }
}
