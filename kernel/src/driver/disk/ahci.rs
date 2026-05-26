pub mod bitfields;
pub mod fis;
pub mod port;

use core::ptr::null_mut;

use alloc::sync::Arc;
use bit_field::BitField;

use fioxa_rpc::{disk_capnp, server::RPCServer, service::ServiceExecutor};
use kernel_sys::{
    syscall::{sys_map, sys_process_spawn_thread, sys_vmo_mmap_create},
    types::VMMapFlags,
};
use kernel_userspace::{channel::Channel, mutex::Mutex};
use volatile::Volatile;

use crate::pci::PCIHeaderCommon;

use self::{
    bitfields::HBAPRDTEntry,
    port::{Port, PortType},
};

const HBA_PORT_DEV_PRESENT: u8 = 0x3;
const HBA_PORT_IPM_ACTIVE: u8 = 0x1;
const SATA_SIG_ATAPI: u32 = 0xEB140101;
const SATA_SIG_ATA: u32 = 0x00000101;
const SATA_SIG_SEMB: u32 = 0xC33C0101;
const SATA_SIG_PM: u32 = 0x96690101;

const HBA_PX_CMD_ST: u32 = 0x0001;
const HBA_PX_CMD_FREE: u32 = 0x0010;
const HBA_PX_CMD_FR: u32 = 0x4000;
const HBA_PX_CMD_CR: u32 = 0x8000;

pub struct AHCIDriver {}

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

    pub fn create(device: PCIHeaderCommon) {
        let pci_device = device;
        trace!("AHCI: {}", pci_device.get_device_id());
        let header0 = unsafe { pci_device.get_as_header0() };

        trace!("BAR5: {}", header0.get_bar(5));
        let abar = header0.get_bar(5);

        let abar_vaddr = unsafe {
            let vmo = sys_vmo_mmap_create(abar as *mut (), 0x1000);
            sys_map(Some(vmo), VMMapFlags::WRITEABLE, null_mut(), 0x1000).unwrap()
        };

        let abar = unsafe { &mut *(abar_vaddr as *mut HBAMemory) };

        let ports_implemented = abar.ports_implemented.read();

        for (i, port) in (abar.ports).iter_mut().enumerate() {
            if ports_implemented.get_bit(i) {
                let port_type = Self::check_port_type(port);

                trace!("SATA: {port_type:?}");

                if port_type == PortType::SATA {
                    let port = ArcPort {
                        port: Arc::new(Mutex::new(Port::new(port))),
                        offset: 0,
                        length: u64::MAX,
                        write: true,
                    };
                    sys_process_spawn_thread(move || {
                        ServiceExecutor::with_name("DISK", |c| {
                            let port = port.clone();
                            sys_process_spawn_thread(move || RPCServer::new(c, port).run());
                        })
                        .run()
                        .unwrap();
                    });
                }
            }
        }
    }
}

#[derive(Clone)]
struct ArcPort {
    port: Arc<Mutex<Port>>,
    offset: u64,
    length: u64,
    write: bool,
}

impl fioxa_rpc::disk::Service for ArcPort {
    fn read<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, disk_capnp::read::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, disk_capnp::read_resp::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        if req.get_count() as u64 + req.get_sector() > self.length {
            return Err(capnp::Error::failed("out of bounds".into()));
        }
        let start = req.get_sector() + self.offset;
        let buffer = res.init_data(req.get_count() * 512);

        self.port.lock().read(start, req.get_count(), buffer);
        Ok(())
    }

    fn identify<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, disk_capnp::identify::Owned>,
        req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, disk_capnp::read_resp::Owned>,
        res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        self.port
            .lock()
            .identify(req, req_handles, res, res_handles)
    }

    fn write<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, disk_capnp::write::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        _res: fioxa_rpc::OwnedBuilder<'a, disk_capnp::write_resp::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        if !self.write {
            return Err(capnp::Error::failed("this capability cannot write".into()));
        }

        let data = req.get_data()?;

        if data.is_empty() {
            return Ok(());
        }

        if !data.len().is_multiple_of(512) {
            return Err(capnp::Error::failed(
                "data must be a multiple of sector size (512)".into(),
            ));
        }

        let count = data.len() / 512;

        if count as u64 + req.get_sector() > self.length {
            return Err(capnp::Error::failed("out of bounds".into()));
        }

        self.port.lock().write(self.offset + req.get_sector(), data);
        Ok(())
    }

    fn restrict<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, disk_capnp::restrict::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, disk_capnp::restrict_resp::Owned>,
        res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let (offset, length) = if req.get_length() > 0 {
            let new_offset = self.offset + req.get_offset();
            let new_len_abs = new_offset + req.get_length();

            if new_len_abs > self.offset + self.length {
                return Err(capnp::Error::failed(
                    "request narrowing out of bounds".into(),
                ));
            }
            (new_offset, req.get_length())
        } else {
            (self.offset, self.length)
        };

        let port = ArcPort {
            port: self.port.clone(),
            offset,
            length,
            write: req.get_write(),
        };

        let (left, right) = Channel::new();
        sys_process_spawn_thread(move || {
            ServiceExecutor::from_channel(right, |c| {
                let port = port.clone();
                sys_process_spawn_thread(move || RPCServer::new(c, port).run());
            })
            .run()
            .unwrap();
        });

        res_handles.add(res.init_handle(), left.into_inner());

        Ok(())
    }
}
