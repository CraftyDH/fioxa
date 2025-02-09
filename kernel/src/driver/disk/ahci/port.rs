use core::{
    mem::{MaybeUninit, size_of},
    pin::Pin,
    time::Duration,
};

use alloc::boxed::Box;
use kernel_sys::syscall::sys_sleep;
use kernel_userspace::disk::ata::ATADiskIdentify;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    driver::disk::{
        DiskDevice,
        ahci::{
            HBACommandTable,
            fis::{FISTYPE, FisRegH2D},
        },
    },
};

use super::{
    HBA_PX_CMD_CR, HBA_PX_CMD_FR, HBA_PX_CMD_FRE, HBA_PX_CMD_ST, HBAPort,
    bitfields::HBACommandHeader, fis::ReceivedFis,
};

#[derive(Debug, PartialEq)]
pub enum PortType {
    None = 0,
    SATA = 1,
    SEMB = 2,
    PM = 3,
    SATAPI = 4,
}

pub const PRDT_LENGTH: usize = 8;

#[allow(dead_code)]
pub struct Port {
    hba_port: &'static mut HBAPort,
    received_fis: Pin<Box<ReceivedFis>>,
    cmd_list: Pin<Box<[HBACommandHeader; 32]>>,
    cmd_tables: Pin<Box<[HBACommandTable<PRDT_LENGTH>; 32]>>,
}

unsafe fn get_phys_addr_from_vaddr(address: u64) -> Option<u64> {
    unsafe {
        let thread = CPULocalStorageRW::get_current_task();
        let mem = thread.process().memory.lock();
        mem.page_mapper.get_phys_addr_from_vaddr(address)
    }
}

impl Port {
    pub fn new(port: &'static mut HBAPort) -> Self {
        unsafe {
            Self::stop_cmd(port);

            let received_fis: Pin<Box<ReceivedFis>> =
                Box::into_pin(Box::new_uninit().assume_init());
            let rfis_addr =
                get_phys_addr_from_vaddr(&*received_fis.as_ref() as *const ReceivedFis as u64)
                    .unwrap();

            let mut cmd_list: Pin<Box<[HBACommandHeader; 32]>> =
                Box::into_pin(Box::new_uninit().assume_init());
            let cmd_list_addr = get_phys_addr_from_vaddr(cmd_list.as_ptr() as u64).unwrap();

            port.command_list_base.write(cmd_list_addr);
            port.fis_base_address.write(rfis_addr);

            let cmd_tables: Pin<Box<[HBACommandTable<PRDT_LENGTH>; 32]>> =
                Box::into_pin(Box::new_uninit().assume_init());
            let cmd_tables_addr: u64 =
                get_phys_addr_from_vaddr(cmd_tables.as_ptr() as u64).unwrap();

            for i in 0..32 {
                cmd_list[i].set_prdt_length(0);
                cmd_list[i].set_command_table_base_address(
                    cmd_tables_addr + i as u64 * size_of::<HBACommandTable<PRDT_LENGTH>>() as u64,
                );
            }

            // port.sata_active.write(u32::MAX);

            Self::start_cmd(port);
            Self {
                hba_port: port,
                received_fis,
                cmd_list,
                cmd_tables,
            }
        }
    }

    pub fn find_slot(&mut self) -> u8 {
        let test = self.hba_port.command_issue.read() | self.hba_port.sata_active.read();
        loop {
            for slot in 0..32 {
                if test & (1 << slot) == 0 {
                    return slot;
                }
            }
            sys_sleep(Duration::from_millis(10));
        }
    }

    pub fn start_cmd(port: &mut HBAPort) {
        while port.cmd_sts.read() & HBA_PX_CMD_CR > 0 {
            // yield_now();
        }

        port.cmd_sts.update(|v| *v |= HBA_PX_CMD_FRE);
        port.cmd_sts.update(|v| *v |= HBA_PX_CMD_ST);
    }

    pub fn stop_cmd(port: &mut HBAPort) {
        // Stop port
        port.cmd_sts.update(|x| *x &= !HBA_PX_CMD_ST);
        // LIST_ON
        while port.cmd_sts.read() | HBA_PX_CMD_CR == 1 {}

        port.cmd_sts.update(|x| *x &= !HBA_PX_CMD_FRE);
        while port.cmd_sts.read() | HBA_PX_CMD_FR == 1 {}
    }
}

impl DiskDevice for Port {
    fn read(&mut self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()> {
        // because of alignment we can't ensure a full transfer
        const MAX_SECTORS: usize = (PRDT_LENGTH - 1) * 8;
        if sector_count as usize > MAX_SECTORS {
            todo!("Sectors count of {MAX_SECTORS} is max atm")
        }

        assert!(
            buffer.len() >= sector_count as usize * 512,
            "Buffer is not large enough"
        );

        let sector_low = sector as u32;
        let sector_high = (sector >> 32) as u32;

        let slot = self.find_slot() as usize;

        let cmd_list = &mut self.cmd_list[slot];
        cmd_list.set_command_fis_length((size_of::<FisRegH2D>() / 4) as u8);
        cmd_list.set_write(false); // This is read

        let cmd_table = &mut self.cmd_tables[slot];

        let mut prdt_length = 0;

        let mut ptr_addr = buffer.as_ptr() as u64;

        let left_align_size = (ptr_addr & 0xFFF) as u32;
        let mut bytes_to_read = 512 * sector_count;

        if left_align_size > 0 {
            // Align ptr on prev boundary
            ptr_addr &= !0xFFF;

            let phys_addr = unsafe { get_phys_addr_from_vaddr(ptr_addr).unwrap() };

            // Set the offset back on, since page offsets arn't supper pain yet (Only 4kb pages)
            cmd_table.prdt_entry[0].set_data_base_address(phys_addr + left_align_size as u64);

            cmd_table.prdt_entry[0].set_byte_count(0xFFF - left_align_size);
            // cmd_table.prdt_entry[0].set_interrupt_on_completion(true);
            prdt_length = 1;
            // Might have requested less than 0x1000 bytes
            bytes_to_read = bytes_to_read.saturating_sub(0x1000 - left_align_size);
            ptr_addr += 0x1000;
        }

        while bytes_to_read > 0x1000 {
            let phys_addr = unsafe { get_phys_addr_from_vaddr(ptr_addr).unwrap() };
            cmd_table.prdt_entry[prdt_length].set_data_base_address(phys_addr);
            // Read read of bytes
            cmd_table.prdt_entry[prdt_length].set_byte_count(0xFFF);
            bytes_to_read -= 0x1000;
            ptr_addr += 0x1000;
            prdt_length += 1;
        }

        if bytes_to_read > 0 {
            let phys_addr = unsafe { get_phys_addr_from_vaddr(ptr_addr).unwrap() };

            cmd_table.prdt_entry[prdt_length].set_data_base_address(phys_addr);
            // Read read of bytes
            cmd_table.prdt_entry[prdt_length].set_byte_count(bytes_to_read - 1);
            prdt_length += 1;
        }

        cmd_list.set_prdt_length(prdt_length as u16);

        let cmd_fis = unsafe { &mut *(cmd_table.command_fis.as_mut_ptr() as *mut FisRegH2D) };
        cmd_fis.set_fis_type(FISTYPE::REGH2D as u8);
        cmd_fis.set_control(1); // COMMAND

        const ATA_CMD_READ_DMA_EX: u8 = 0x25;
        cmd_fis.set_command(ATA_CMD_READ_DMA_EX);
        cmd_fis.set_command_control(true);

        cmd_fis.set_lba0(sector_low as u8);
        cmd_fis.set_lba1((sector_low >> 8) as u8);
        cmd_fis.set_lba2((sector_low >> 16) as u8);

        cmd_fis.set_lba3(sector_high as u8);
        cmd_fis.set_lba4((sector_high >> 8) as u8);
        cmd_fis.set_lba5((sector_high >> 16) as u8);

        cmd_fis.set_device_register(1 << 6); // LBA mode

        cmd_fis.set_countl((sector_count & 0xFF) as u8);
        cmd_fis.set_counth(((sector_count >> 8) & 0xFF) as u8);

        let mut spin = 100_000;

        while ((self.hba_port.task_file_data.read() & (0x80 | 0x08)) > 0) && spin > 0 {
            spin -= 1;
            // yield_now();
        }
        if spin == 0 {
            error!("Port is hung");
            return None;
        }

        self.hba_port.command_issue.write(1 << slot);
        loop {
            // yield_now();
            if self.hba_port.command_issue.read() & (1 << slot) == 0 {
                break;
            }
            if self.hba_port.interrupt_status.read() & (1 << 30) > 0 {
                debug!("Err");
                return None;
                // Read error
            }
        }
        if self.hba_port.interrupt_status.read() & (1 << 30) > 0 {
            debug!("Err");
            return None; // Read error
        }

        Some(())
    }

    fn write(&mut self, _sector: usize, _sector_count: u32, _buffer: &mut [u8]) -> Option<()> {
        todo!()
    }

    fn identify(&mut self) -> Box<ATADiskIdentify> {
        self.hba_port.interrupt_status.write(0xFFFFFFFF);
        let slot = self.find_slot() as usize;

        let cmd_list = &mut self.cmd_list[slot];
        cmd_list.set_command_fis_length((size_of::<FisRegH2D>() / 4) as u8);
        cmd_list.set_write(false); // This is read

        let cmd_table = &mut self.cmd_tables[slot];

        let identify: Box<MaybeUninit<ATADiskIdentify>> = Box::new_uninit();

        // TODO: Will probably break if buffer ever spans two non continuous pages
        let phys_addr = unsafe { get_phys_addr_from_vaddr(identify.as_ptr() as u64).unwrap() };

        cmd_table.prdt_entry[0].set_data_base_address(phys_addr);
        cmd_table.prdt_entry[0].set_byte_count(size_of::<ATADiskIdentify>() as u32 - 1);

        cmd_list.set_prdt_length(1);

        let cmd_fis = unsafe { &mut *(cmd_table.command_fis.as_mut_ptr() as *mut FisRegH2D) };
        cmd_fis.set_fis_type(FISTYPE::REGH2D as u8);
        // cmd_fis.set_control(1);

        cmd_fis.set_command(0xec); // Ident
        cmd_fis.set_countl(0);
        cmd_fis.set_command_control(true);

        let mut spin = 0;

        while ((self.hba_port.task_file_data.read() & (0x80 | 0x08)) > 0) && spin < 1000000 {
            spin += 1;
        }
        if spin == 1000000 {
            todo!("Port is hung");
        }

        self.hba_port.command_issue.write(1 << slot);

        let mut i = 100_000;
        while i > 0 {
            // yield_now();
            // println!("Reading...: {:b}", self.hba_port.command_issue.read());
            if self.hba_port.command_issue.read() & (1 << slot) == 0 {
                break;
            }
            if self.hba_port.interrupt_status.read() & (1 << 30) > 0 {
                debug!("Error reading");
                break;
            }
            i -= 1;
        }
        if i == 0 {
            todo!("Failed to read identify")
        }
        unsafe { identify.assume_init() }
    }
}
