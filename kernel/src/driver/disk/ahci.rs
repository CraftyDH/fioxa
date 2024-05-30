pub mod bitfields;
pub mod fis;
pub mod port;

use alloc::{sync::Arc, vec::Vec};
use bit_field::BitField;

use spin::Mutex;
use volatile::Volatile;

use crate::{
    driver::{disk::DiskDevice, driver::Driver},
    paging::{
        get_uefi_active_mapper,
        page_table_manager::{Mapper, Page, Size4KB},
        MemoryMappingFlags,
    },
    pci::{PCIHeader0, PCIHeaderCommon},
};

use self::{
    bitfields::HBAPRDTEntry,
    port::{Port, PortType},
};

use super::DiskBusDriver;

const HBA_PORT_DEV_PRESENT: u8 = 0x3;
const HBA_PORT_IPM_ACTIVE: u8 = 0x1;
const SATA_SIG_ATAPI: u32 = 0xEB140101;
const SATA_SIG_ATA: u32 = 0x00000101;
const SATA_SIG_SEMB: u32 = 0xC33C0101;
const SATA_SIG_PM: u32 = 0x96690101;

const HBA_PX_CMD_ST: u32 = 0x0001;
const HBA_PX_CMD_FRE: u32 = 0x0010;
const HBA_PX_CMD_FR: u32 = 0x4000;
const HBA_PX_CMD_CR: u32 = 0x8000;

pub struct AHCIDriver {
    #[allow(dead_code)]
    pci_device: PCIHeader0,
    // abar: HBAMemory,
    ports: [Option<Arc<Mutex<Port>>>; 32],
}

#[repr(C)]
pub struct HBACommandTable<const N: usize> {
    command_fis: [u8; 64],
    atapi_command: [u8; 16],
    rsv: [u8; 48],
    prdt_entry: [HBAPRDTEntry; N],
}

#[repr(C)]
pub struct HBAPort {
    // do I really have to split into two u32's?
    command_list_base: Volatile<u64>,
    // do I really have to split into two u32's?
    fis_base_address: Volatile<u64>,
    interrupt_status: Volatile<u32>,
    interrupt_enable: Volatile<u32>,
    cmd_sts: Volatile<u32>,
    _rsv0: Volatile<u32>,
    task_file_data: Volatile<u32>,
    signature: Volatile<u32>,
    sata_status: Volatile<u32>,
    sata_control: Volatile<u32>,
    sata_error: Volatile<u32>,
    sata_active: Volatile<u32>,
    command_issue: Volatile<u32>,
    sata_notification: Volatile<u32>,
    fis_switch_control: Volatile<u32>,
    _rsv1: Volatile<[u32; 11]>,
    _vendor: Volatile<[u32; 4]>,
}

#[repr(C)]
pub struct HBAMemory {
    host_capability: Volatile<u32>,
    global_host_control: Volatile<u32>,
    interrupt_status: Volatile<u32>,
    ports_implemented: Volatile<u32>,
    version: Volatile<u32>,
    ccc_control: Volatile<u32>,
    ccc_ports: Volatile<u32>,
    enclosure_management_location: Volatile<u32>,
    enclosure_management_control: Volatile<u32>,
    host_capabilities_extended: Volatile<u32>,
    bios_handof_ctrl_sts: Volatile<u32>,
    _rsv0: [u8; 0x74],
    _vendor: [u8; 0x60],
    ports: [HBAPort; 32],
}

impl AHCIDriver {
    pub fn check_port_type(port: &HBAPort) -> PortType {
        let sata_status = port.sata_status.read();

        let interface_power_management = ((sata_status >> 8) & 0b111) as u8;
        let device_detection = (sata_status & 0b111) as u8;

        if device_detection != HBA_PORT_DEV_PRESENT {
            return PortType::None;
        }
        if interface_power_management != HBA_PORT_IPM_ACTIVE {
            return PortType::None;
        }
        trace!("Port: {:X}", port.signature.read());
        match port.signature.read() {
            SATA_SIG_ATAPI => PortType::SATAPI,
            SATA_SIG_ATA => PortType::SATA,
            SATA_SIG_PM => PortType::PM,
            SATA_SIG_SEMB => PortType::SEMB,
            _ => PortType::None,
        }
    }
}

unsafe impl Send for AHCIDriver {}

impl Driver for AHCIDriver {
    fn new(device: PCIHeaderCommon) -> Option<Self>
    where
        Self: Sized,
    {
        let pci_device = device;
        trace!("AHCI: {}", pci_device.get_device_id());
        let header0 = unsafe { pci_device.get_as_header0() };
        let mut mapper = unsafe { get_uefi_active_mapper() };

        trace!("BAR5: {}", header0.get_bar(5));
        let abar = header0.get_bar(5);

        mapper
            .identity_map_memory(Page::<Size4KB>::new(abar as u64), MemoryMappingFlags::all())
            .unwrap()
            .flush();

        let abar = unsafe { &mut *(header0.get_bar(5) as *mut HBAMemory) };

        let mut ahci = Self {
            pci_device: header0,
            ports: Default::default(),
        };

        let ports_implemented = abar.ports_implemented.read();

        let buffer = &mut [0u8; 512];

        for (i, port) in (abar.ports).iter_mut().enumerate() {
            if ports_implemented.get_bit(i) {
                let port_type = Self::check_port_type(port);

                trace!("SATA: {:?}", port_type);

                if port_type == PortType::SATA {
                    let mut port = Port::new(port);

                    // Test read
                    if port.read(0, 1, buffer).is_some() {
                        ahci.ports[i] = Some(Arc::new(Mutex::new(port)));
                    }
                }
            }
        }

        Some(ahci)
    }

    fn unload(self) -> ! {
        todo!()
    }

    fn interrupt_handler(&mut self) {
        todo!()
    }
}

impl DiskBusDriver for AHCIDriver {
    fn get_disks(&mut self) -> Vec<Arc<Mutex<dyn DiskDevice>>> {
        self.ports
            .clone()
            .into_iter()
            .flatten()
            .map(|a| a as Arc<Mutex<dyn DiskDevice>>)
            .collect()
    }

    fn get_disk_by_id(&mut self, id: usize) -> Option<Arc<Mutex<dyn DiskDevice>>> {
        if let Some(Some(port)) = self.ports.get(id) {
            return Some(port.clone());
        }
        None
    }
}
