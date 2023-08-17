use crate::{
    acpi::FioxaAcpiHandler,
    cpu_localstorage::CPULocalStorageRW,
    driver::{disk::ahci::AHCIDriver, driver::Driver, net::amd_pcnet::amd_pcnet_main},
    fs::FSDRIVES,
    pci::mcfg::get_mcfg,
};

use alloc::{boxed::Box, format, sync::Arc, vec::Vec};

use kernel_userspace::{
    ids::ServiceID,
    service::ServiceMessage,
    syscall::{send_service_message, spawn_process, spawn_thread},
};
use spin::Mutex;
mod express;
mod legacy;
mod mcfg;
mod pci_descriptors;

pub type PCIDriver = Arc<Mutex<dyn Driver + Send>>;

pub trait PCIDevice: Send + Sync {
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

pub fn enumerate_pci(acpi_tables: acpi::AcpiTables<FioxaAcpiHandler>) {
    // Enable interrupts
    // set_handler_and_enable_irq(10, interrupt_handler);
    // set_handler_and_enable_irq(11, interrupt_handler);

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
        enumerate_function(pci_bus, segment, bus, device, function);
    }
}

fn enumerate_function(pci_bus: &mut impl PCIBus, segment: u16, bus: u8, device: u8, function: u8) {
    let pci_header = pci_bus.get_device(segment, bus, device, function);

    if pci_header.get_device_id() == 0 || pci_header.get_device_id() == 0xFFFF {
        return;
    }

    let class = pci_header.get_class() as usize;
    let cls = if class < pci_descriptors::DEVICE_CLASSES.len() {
        pci_descriptors::DEVICE_CLASSES[class]
    } else {
        "Unknown"
    };

    println!(
        "Class: {}, Vendor: {}, Device: {}",
        cls,
        pci_descriptors::get_vendor_name(pci_header.get_vendor_id())
            .unwrap_or(format!("Unknown vendor: {:#X}", { pci_header.get_vendor_id() }).as_str()),
        pci_descriptors::get_device_name(pci_header.get_vendor_id(), pci_header.get_device_id())
            .unwrap_or(format!("Unknown device: {:#X}", { pci_header.get_device_id() }).as_str())
    );

    // Specific drivers
    match pci_header.get_vendor_id() {
        // AMD
        0x1022 => match pci_header.get_device_id() {
            // AM79c973
            0x2000 => {
                println!("AMD PCnet");
                let sid = pci_dev_handler(pci_bus, segment, bus, device, function);
                spawn_process(amd_pcnet_main, &sid.0.to_ne_bytes(), true);
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
                            Some(d) => FSDRIVES.lock().add_device(Box::new(d)),
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
    fn get_device_raw(
        &mut self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
    ) -> Box<dyn PCIDevice>;
}

fn pci_dev_handler(
    pci_bus: &mut impl PCIBus,
    segment: u16,
    bus: u8,
    device: u8,
    function: u8,
) -> ServiceID {
    let mut device = pci_bus.get_device_raw(segment, bus, device, function);
    let sid = kernel_userspace::syscall::service_create();

    spawn_thread(move || loop {
        let mut buf = Vec::new();
        let req =
            kernel_userspace::syscall::receive_service_message_blocking(sid, &mut buf).unwrap();
        let resp = match req.message {
            kernel_userspace::pci::PCIDevCmd::Read(offset) if offset <= 256 => unsafe {
                kernel_userspace::pci::PCIDevCmd::Data(device.read_u32(offset))
            },
            kernel_userspace::pci::PCIDevCmd::Write(offset, data) if offset <= 256 => unsafe {
                device.write_u32(offset, data);
                kernel_userspace::pci::PCIDevCmd::Ack
            },
            _ => kernel_userspace::pci::PCIDevCmd::UnknownCommand,
        };
        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: CPULocalStorageRW::get_current_pid(),
                tracking_number: req.tracking_number,
                destination: kernel_userspace::service::SendServiceMessageDest::ToProcess(
                    req.sender_pid,
                ),
                message: resp,
            },
            &mut buf,
        )
        .unwrap();
    });

    sid
}
