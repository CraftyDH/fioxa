use alloc::boxed::Box;
use spin::Mutex;
use x86_64::instructions::port::Port;

use super::{PCIBus, PCIDeviceCommonHeader};

pub static LEGACY_PCI_COMMAND: Mutex<LegacyPCICommand> = Mutex::new(LegacyPCICommand {
    data_port: Port::new(0xCFC),
    command_port: Port::new(0xCF8),
});

pub struct LegacyPCICommand {
    data_port: Port<u32>,
    command_port: Port<u32>,
}

impl LegacyPCICommand {
    fn read(&mut self, address: u32) -> u32 {
        // Send the device ID to the PCI controller
        unsafe { self.command_port.write(address) };

        // Read the result
        unsafe { self.data_port.read() }
    }

    #[allow(dead_code)]
    fn write(&mut self, address: u32, value: u32) {
        // Send the device ID to the PCI controller
        unsafe { self.command_port.write(address) };

        // Read the result
        unsafe { self.data_port.write(value) }
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
    fn get_device(
        &mut self,
        _segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDeviceCommonHeader> {
        let new_header = PCIDeviceCommonHeaderLegacy::new(bus, device, function);
        Box::new(new_header)
    }
}

/// Refer to the ubove struct PCIDeviceCommonHeaderExpress
/// for the ordering of the blocks
pub struct PCIDeviceCommonHeaderLegacy {
    base_address: u32,
    block0: Option<u32>,
    block1: Option<u32>,
    block2: Option<u32>,
    block3: Option<u32>,
}

impl PCIDeviceCommonHeaderLegacy {
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            base_address: LegacyPCI::get_address(bus, device, function),
            block0: None,
            block1: None,
            block2: None,
            block3: None,
        }
    }

    fn get_block0(&mut self) -> u32 {
        if let None = self.block0 {
            self.block0 = Some(LEGACY_PCI_COMMAND.lock().read(self.base_address));
        }
        self.block0.unwrap()
    }

    fn get_block1(&mut self) -> u32 {
        if let None = self.block1 {
            self.block1 = Some(LEGACY_PCI_COMMAND.lock().read(self.base_address + 0x4));
        }
        self.block1.unwrap()
    }

    fn get_block2(&mut self) -> u32 {
        if let None = self.block2 {
            self.block2 = Some(LEGACY_PCI_COMMAND.lock().read(self.base_address + 0x8));
        }
        self.block2.unwrap()
    }

    fn get_block3(&mut self) -> u32 {
        if let None = self.block3 {
            self.block3 = Some(LEGACY_PCI_COMMAND.lock().read(self.base_address + 0xC));
        }
        self.block3.unwrap()
    }
}

impl PCIDeviceCommonHeader for PCIDeviceCommonHeaderLegacy {
    fn get_vendor_id(&mut self) -> u16 {
        let block = self.get_block0();
        (block & 0xFFFF) as u16
    }

    fn get_device_id(&mut self) -> u16 {
        let block = self.get_block0();
        ((block >> 16) & 0xFFFF) as u16
    }

    fn get_command(&mut self) -> u16 {
        let block = self.get_block1();
        (block & 0xFFFF) as u16
    }

    fn get_status(&mut self) -> u16 {
        let block = self.get_block1();
        ((block >> 16) & 0xFFFF) as u16
    }

    // Block 2
    // Class | Subclass | Prog If | Revison
    fn get_revision_id(&mut self) -> u8 {
        (self.get_block2() & 0xFF) as u8
    }

    fn get_prog_if(&mut self) -> u8 {
        ((self.get_block2() >> 8) & 0xFF) as u8
    }

    fn get_subclass(&mut self) -> u8 {
        ((self.get_block2() >> 16) & 0xFF) as u8
    }

    fn get_class(&mut self) -> u8 {
        ((self.get_block2() >> 24) & 0xFF) as u8
    }

    // Block 3
    // BIST | Header Time | Latency Timer | Cache line size
    fn get_cache_line_size(&mut self) -> u8 {
        (self.get_block3() & 0xFF) as u8
    }

    fn get_latency_timer(&mut self) -> u8 {
        ((self.get_block3() >> 8) & 0xFF) as u8
    }

    fn get_header_type(&mut self) -> u8 {
        ((self.get_block3() >> 16) & 0xFF) as u8
    }

    fn get_bist(&mut self) -> u8 {
        ((self.get_block3() >> 24) & 0xFF) as u8
    }
}
