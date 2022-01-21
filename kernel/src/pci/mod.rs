use crate::{acpi::FioxaAcpiHandler, pci::mcfg::get_mcfg};

use alloc::{boxed::Box, format};

mod express;
mod legacy;
mod mcfg;
mod pci_descriptors;
mod port_based;

pub fn enumerate_pci(acpi_tables: &acpi::AcpiTables<FioxaAcpiHandler>) {
    // Get MCFG;
    let mcfg = get_mcfg(acpi_tables);

    if let Err(e) = &mcfg {
        println!("Error with getting MCFG table: {:?}", e)
    }
    if let Ok(mcfg) = &mcfg {
        println!("Enumerating MCFG");
        let mut pci_bus = express::ExpressPCI::new(mcfg);
        for entry in mcfg.entries() {
            for bus_number in entry.bus_number_start..entry.bus_number_end {
                enumerate_bus(&mut pci_bus, entry.pci_segment_group, bus_number)
            }
        }
    }

    // Enumerate using legacy
    {
        println!("Enum PCI using legacy");
        let mut pci_bus = legacy::LegacyPCI {};
        for bus_number in 0..255 {
            enumerate_bus(&mut pci_bus, 0, bus_number)
        }
    }
}

fn enumerate_bus(pci_bus: &mut impl PCIBus, segment: u16, bus: u8) {
    let mut pci_header = pci_bus.get_device(segment, bus, 0, 0);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }
    for device in 0..32 {
        enumerate_device(pci_bus, segment, bus, device)
    }
}

fn enumerate_device(pci_bus: &mut impl PCIBus, segment: u16, bus: u8, device: u8) {
    let mut pci_header = pci_bus.get_device(segment, bus, device, 0);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }
    for function in 0..8 {
        enumerate_function(pci_bus, segment, bus, device, function)
    }
}

fn enumerate_function(pci_bus: &mut impl PCIBus, segment: u16, bus: u8, device: u8, function: u8) {
    let mut pci_header = pci_bus.get_device(segment, bus, device, function);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }

    println!(
        "Class: {}, Vendor: {}, Device: {}",
        pci_descriptors::DEVICE_CLASSES[pci_header.get_class() as usize],
        pci_descriptors::get_vendor_name(pci_header.get_vendor_id())
            .unwrap_or(&format!("Unknown vendor: {:#X}", { pci_header.get_vendor_id() }).as_str()),
        pci_descriptors::get_device_name(pci_header.get_vendor_id(), pci_header.get_device_id())
            .unwrap_or(&format!("Unknown device: {:#X}", { pci_header.get_device_id() }).as_str())
    );

    match pci_header.get_class() {
        // Mass storage
        0x01 => match pci_header.get_subclass() {
            // Serial ATA
            0x06 => {
                match pci_header.get_prog_if() {
                    // AHCI 1.0 device
                    0x01 => {
                        println!("AHCI");
                        // AHCIDriver::new(mapper, pci_header);
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
    fn get_device(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDeviceCommonHeader>;
}

pub trait PCIDeviceCommonHeader {
    fn get_vendor_id(&mut self) -> u16;
    fn get_device_id(&mut self) -> u16;
    fn get_command(&mut self) -> u16;
    fn get_status(&mut self) -> u16;
    fn get_revision_id(&mut self) -> u8;
    fn get_prog_if(&mut self) -> u8;
    fn get_subclass(&mut self) -> u8;
    fn get_class(&mut self) -> u8;
    fn get_cache_line_size(&mut self) -> u8;
    fn get_latency_timer(&mut self) -> u8;
    fn get_header_type(&mut self) -> u8;
    fn get_bist(&mut self) -> u8;
}
