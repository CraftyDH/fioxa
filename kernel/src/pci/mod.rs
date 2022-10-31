use crate::{
    acpi::FioxaAcpiHandler,
    driver::{disk::ahci::AHCIDriver, driver::Driver, net::amd_pcnet::PCNET},
    fs::ROOTFS,
    interrupts::hardware::set_handler_and_enable_irq,
    net::ethernet::ETHERNET,
    pci::mcfg::get_mcfg,
};

use alloc::{boxed::Box, format, sync::Arc, vec::Vec};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

mod express;
mod legacy;
mod mcfg;
mod pci_descriptors;

pub type PCIDriver = Arc<Mutex<dyn Driver + Send>>;

lazy_static::lazy_static! {
    pub static ref PCI_INTERRUPT_DEVICES: Mutex<Vec<PCIDriver>> = Mutex::new(Vec::new());
}

// TODO: Change functionality depeding on interrupt number
pub fn interrupt_handler(_: InterruptStackFrame) {
    // For each device check if it had the interrupt
    PCI_INTERRUPT_DEVICES.try_lock().and_then(|mut d| {
        for device in d.iter_mut() {
            device
                .try_lock()
                .and_then(|mut d| Some(d.interrupt_handler()));
        }
        Some(())
    });
}

pub trait PCIDevice {
    unsafe fn read_u8(&self, offset: u32) -> u8;
    unsafe fn read_u16(&self, offset: u32) -> u16;
    unsafe fn read_u32(&self, offset: u32) -> u32;

    unsafe fn write_u8(&mut self, offset: u32, data: u8);
    unsafe fn write_u16(&mut self, offset: u32, data: u16);
    unsafe fn write_u32(&mut self, offset: u32, data: u32);
}

pub struct PCIHeaderCommon {
    device: Arc<dyn PCIDevice>,
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
    device: Arc<dyn PCIDevice>,
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
            let address = (bar & 0xFFFFFFFC).try_into().unwrap();
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

pub fn enumerate_pci(acpi_tables: acpi::AcpiTables<FioxaAcpiHandler>) {
    // Enable interrupts
    set_handler_and_enable_irq(10, interrupt_handler);
    set_handler_and_enable_irq(11, interrupt_handler);

    // Get MCFG
    let mcfg = get_mcfg(&acpi_tables);

    // Enumerate PCI using mcfg;
    match &mcfg {
        Ok(mcfg) => {
            println!("Enumerating PCI using MCFG...");
            let mut pci_bus = express::ExpressPCI::new(mcfg);
            for entry in mcfg.entries() {
                for bus_number in entry.bus_number_start..entry.bus_number_end {
                    enumerate_bus(&mut pci_bus, entry.pci_segment_group, bus_number)
                }
            }
            return;
        }
        Err(e) => println!("Error with getting MCFG table: {:?}", e),
    }
    // Enumerate using legacy port based
    {
        println!("Enumerating PCI using legacy ports...");
        let mut pci_bus = legacy::LegacyPCI {};
        for bus_number in 0..255 {
            enumerate_bus(&mut pci_bus, 0, bus_number)
        }
    }
}

fn enumerate_bus(pci_bus: &mut impl PCIBus, segment: u16, bus: u8) {
    let pci_header = pci_bus.get_device(segment, bus, 0, 0);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }
    for device in 0..32 {
        enumerate_device(pci_bus, segment, bus, device)
    }
}

fn enumerate_device(pci_bus: &mut impl PCIBus, segment: u16, bus: u8, device: u8) {
    let pci_header = pci_bus.get_device(segment, bus, device, 0);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }
    for function in 0..8 {
        enumerate_function(pci_bus, segment, bus, device, function)
    }
}

fn enumerate_function(pci_bus: &mut impl PCIBus, segment: u16, bus: u8, device: u8, function: u8) {
    let pci_header = pci_bus.get_device(segment, bus, device, function);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }

    let class = pci_header.get_class() as usize;
    let cls;
    if class < pci_descriptors::DEVICE_CLASSES.len() {
        cls = pci_descriptors::DEVICE_CLASSES[class]
    } else {
        cls = "Unknown";
    }

    println!(
        "Class: {}, Vendor: {}, Device: {}",
        cls,
        pci_descriptors::get_vendor_name(pci_header.get_vendor_id())
            .unwrap_or(&format!("Unknown vendor: {:#X}", { pci_header.get_vendor_id() }).as_str()),
        pci_descriptors::get_device_name(pci_header.get_vendor_id(), pci_header.get_device_id())
            .unwrap_or(&format!("Unknown device: {:#X}", { pci_header.get_device_id() }).as_str())
    );

    // Specific drivers
    match pci_header.get_vendor_id() {
        // AMD
        0x1022 => match pci_header.get_device_id() {
            // AM79c973
            0x2000 => {
                println!("AMD PCnet");
                let driv = Arc::new(Mutex::new(PCNET::new(pci_header).unwrap()));
                PCI_INTERRUPT_DEVICES.lock().push(driv.clone());

                ETHERNET.lock().new_device(driv);
                return;
            }
            _ => (),
        },
        _ => (),
    }

    // General drivers
    match pci_header.get_class() {
        // Mass storage
        0x01 => match pci_header.get_subclass() {
            // Serial ATA
            0x06 => {
                match pci_header.get_prog_if() {
                    // AHCI 1.0 device
                    0x01 => {
                        println!("AHCI");
                        match AHCIDriver::new(pci_header) {
                            Some(d) => ROOTFS.lock().add_device(Box::new(d)),
                            None => {
                                println!("AHCI Driver failed to init.");
                            }
                        };
                    }
                    _ => (),
                }
            }
            _ => (),
        },
        _ => (),
    }
}

trait PCIBus {
    fn get_device(&mut self, segment: u16, bus: u8, device: u8, function: u8) -> PCIHeaderCommon;
}
