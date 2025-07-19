use crate::{
    acpi::FioxaAcpiHandler,
    bootfs::early_bootfs_get,
    driver::{Driver, disk::ahci::AHCIDriver},
    elf,
    mutex::Spinlock,
    scheduling::process::ProcessReferences,
};

use alloc::{boxed::Box, sync::Arc};

use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    ipc::IPCChannel,
    pci::{PCIDeviceExecutor, PCIDeviceImpl},
    process::INIT_HANDLE_SERVICE,
};
use mcfg::MCFG;
mod express;
mod legacy;
mod mcfg;
mod pci_descriptors;

pub type PCIDriver = Arc<Spinlock<dyn Driver + Send>>;

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
    let mcfg = acpi_tables.find_table::<MCFG>();

    // Enumerate PCI using mcfg;
    match &mcfg {
        Ok(mcfg) => {
            debug!("Enumerating PCI using MCFG...");
            let mut pci_bus = express::ExpressPCI::new(mcfg);
            for entry in mcfg.entries() {
                for bus_number in entry.bus_number_start..entry.bus_number_end {
                    enumerate_bus(&mut pci_bus, entry.pci_segment_group, bus_number)
                }
            }
            return;
        }
        Err(e) => error!("Error with getting MCFG table: {e:?}"),
    }
    // Enumerate using legacy port based
    {
        debug!("Enumerating PCI using legacy ports...");
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

#[allow(clippy::single_match)]
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

    info!(
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
                debug!("AMD PCnet");
                let sid = pci_dev_handler(pci_bus, segment, bus, device, function);

                elf::load_elf(early_bootfs_get("amd_pcnet").unwrap())
                    .unwrap()
                    .references(ProcessReferences::from_refs(&[
                        **INIT_HANDLE_SERVICE.lock().clone_init_service().handle(),
                        **sid.handle(),
                    ]))
                    .privilege(crate::scheduling::process::ProcessPrivilege::KERNEL)
                    .build();
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
                        debug!("AHCI");
                        AHCIDriver::create(pci_header);
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

impl PCIDeviceImpl for Box<dyn PCIDevice> {
    fn read(&mut self, offset: u32) -> u32 {
        unsafe { self.read_u32(offset) }
    }

    fn write(&mut self, offset: u32, data: u32) {
        unsafe { self.write_u32(offset, data) }
    }
}

fn pci_dev_handler(
    pci_bus: &mut impl PCIBus,
    segment: u16,
    bus: u8,
    device: u8,
    function: u8,
) -> Channel {
    let device = pci_bus.get_device_raw(segment, bus, device, function);
    let (left, right) = Channel::new();
    sys_process_spawn_thread(move || {
        match PCIDeviceExecutor::new(IPCChannel::from_channel(left), device).run() {
            Ok(()) => (),
            Err(e) => warn!("error handling  service: {e}"),
        }
    });

    right
}
