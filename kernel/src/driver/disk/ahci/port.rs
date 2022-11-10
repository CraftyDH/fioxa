use core::mem::size_of;

use alloc::{slice, vec::Vec};
use bit_field::BitField;

use crate::{
    driver::disk::ahci::{
        fis::{FisRegH2D, FISTYPE},
        HBACommandHeader, HBACommandTable,
    },
    paging::{
        get_uefi_active_mapper,
        page_allocator::{free_page, request_page},
        page_table_manager::ident_map_curr_process,
    },
    syscall::yield_now,
};

use super::{fis::ReceivedFis, HBAPort, HBA_PxCMD_CR, HBA_PxCMD_FR, HBA_PxCMD_FRE, HBA_PxCMD_ST};

#[derive(Debug, PartialEq)]
pub enum PortType {
    None = 0,
    SATA = 1,
    SEMB = 2,
    PM = 3,
    SATAPI = 4,
}

pub struct Port<'p> {
    hba_port: &'p mut HBAPort,
    received_fis: &'static mut ReceivedFis,
    cmd_list: &'static mut [HBACommandHeader],
    cmd_table_buffers: Vec<u64>,
}

impl Drop for Port<'_> {
    fn drop(&mut self) {
        free_page(self.received_fis as *const ReceivedFis as u64).unwrap();
        free_page(&self.cmd_list[0] as *const HBACommandHeader as u64).unwrap();
        // Each 16 chunks are on same page
        for i in &self.cmd_table_buffers {
            free_page(*i).unwrap();
        }
    }
}

impl<'p> Port<'p> {
    pub fn new(port: &'p mut HBAPort) -> Self {
        Self::stop_cmd(port);

        let rfis_addr = request_page().unwrap();
        ident_map_curr_process(rfis_addr, true);
        let cmd_list_addr = request_page().unwrap();
        ident_map_curr_process(cmd_list_addr, true);

        let cmd_list =
            unsafe { slice::from_raw_parts_mut(cmd_list_addr as *mut HBACommandHeader, 32) };

        let received_fis = unsafe { &mut *(rfis_addr as *mut ReceivedFis) };

        port.command_list_base.write(cmd_list_addr as u32);
        port.command_list_base_upper
            .write((cmd_list_addr >> 32) as u32);
        port.fis_base_address.write(rfis_addr as u32);

        port.fis_base_address_upper.write((rfis_addr >> 32) as u32);

        let mut cmd_table_buffers = Vec::with_capacity(32);

        for c in 0..=1 {
            let command_table_addr = request_page().unwrap();
            ident_map_curr_process(command_table_addr, true);
            cmd_table_buffers.push(command_table_addr);

            for i in 0..16 {
                let index = i * c;
                // 8 PRDTS's per command table
                // = 256 bytes per cammand table
                cmd_list[index].set_prdt_length(8);

                let address = command_table_addr + i as u64 * 256;
                cmd_list[index].set_command_table_base_address(address);
                // cmd_list[index].set_command_table_base_address_upper((address >> 32) as u32);
            }
        }

        port.sata_active.write(u32::MAX);

        Self::start_cmd(port);
        Self {
            hba_port: port,
            received_fis,
            cmd_list,
            cmd_table_buffers,
        }
    }

    pub fn find_slot(&mut self) -> u8 {
        let test = self.hba_port.command_issue.read();
        for slot in 0..32 {
            if test & (1 << slot) == 0 {
                return slot;
            }
        }
        unimplemented!();
    }

    pub fn identify(&mut self) -> Vec<u8> {
        let slot = self.find_slot() as usize;

        let cmd_header = unsafe {
            &mut *(self.hba_port.command_list_base.read() as *mut [HBACommandHeader; 32])
        };
        let cmd_table =
            unsafe { &mut *(cmd_header[0].command_table_base_address() as *mut HBACommandTable) };

        let buffer: Vec<u8> = vec![0; 512];

        let addr = Self::get_address(buffer.as_ptr() as u64);

        cmd_table.prdt_entry[0].set_data_base_address(addr);
        cmd_table.prdt_entry[0].set_byte_count(512 - 1);

        cmd_header[slot].set_prdt_length(1);
        cmd_header[slot].set_command_fis_length(4);

        let cmd_fis = unsafe { &mut *(cmd_table.command_fis.as_mut_ptr() as *mut FisRegH2D) };
        cmd_fis.set_fis_type(FISTYPE::REGH2D as u8);
        cmd_fis.set_command(0xec); // Ident
        cmd_fis.set_countl(1);
        cmd_fis.set_command_control(true);
        self.hba_port.command_issue.write(1 << slot);

        loop {
            yield_now();
            println!("Reading...: {:b}", self.hba_port.command_issue.read());
            if !self.hba_port.command_issue.read().get_bit(slot) {
                break;
            }
        }
        println!("Port: {:?}", buffer);
        buffer
    }

    pub fn start_cmd(port: &mut HBAPort) {
        while port.cmd_sts.read() & HBA_PxCMD_CR > 0 {
            yield_now();
        }

        port.cmd_sts.update(|v| *v |= HBA_PxCMD_FRE);
        port.cmd_sts.update(|v| *v |= HBA_PxCMD_ST);
    }

    pub fn stop_cmd(port: &mut HBAPort) {
        // Stop port
        port.cmd_sts.update(|x| *x &= !HBA_PxCMD_ST);
        // LIST_ON
        while port.cmd_sts.read() | HBA_PxCMD_CR == 1 {}

        port.cmd_sts.update(|x| *x &= !HBA_PxCMD_FRE);
        while port.cmd_sts.read() | HBA_PxCMD_FR == 1 {}
    }

    pub fn get_address(vaddr: u64) -> u64 {
        let mapper = unsafe { get_uefi_active_mapper() };
        let addr = mapper.get_phys_addr(vaddr & !0xFFF as u64).unwrap();
        addr + vaddr % 0x1000
    }

    pub fn read(&mut self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()> {
        if sector_count > 56 {
            todo!("Sectors count of 64 is max atm")
        }
        let sector_low = sector as u32;
        let sector_high = (sector >> 32) as u32;

        let slot = self.find_slot() as usize;

        let cmd_list = &mut self.cmd_list[slot];
        cmd_list.set_command_fis_length((size_of::<FisRegH2D>() / 4) as u8);
        cmd_list.set_write(false); // This is read

        let cmd_table =
            unsafe { &mut *(cmd_list.command_table_base_address() as *mut HBACommandTable) };

        let mapper = unsafe { get_uefi_active_mapper() };

        let mut prdt_length = 0;

        let mut ptr_addr = buffer.as_ptr() as u64;

        let left_align_size = (ptr_addr & 0xFFF) as u32;
        let mut bytes_to_read = 512 * sector_count;

        if left_align_size > 0 {
            // Align ptr on prev boundary
            ptr_addr = ptr_addr & !0xFFF;

            let phys_addr = mapper.get_phys_addr(ptr_addr).unwrap();
            // Set the offset back on, since page offsets arn't supper pain yet (Only 4kb pages)
            cmd_table.prdt_entry[0]
                .set_data_base_address(Self::get_address(phys_addr) + left_align_size as u64);

            cmd_table.prdt_entry[0].set_byte_count(0xFFF - left_align_size);
            // cmd_table.prdt_entry[0].set_interrupt_on_completion(true);
            prdt_length = 1;
            // Might have requested less than 0x1000 bytes
            bytes_to_read = bytes_to_read.saturating_sub(0x1000 - left_align_size);
            ptr_addr += 0x1000;
        }

        while bytes_to_read > 0x1000 {
            let phys_addr = mapper.get_phys_addr(ptr_addr).unwrap();
            cmd_table.prdt_entry[prdt_length].set_data_base_address(Self::get_address(phys_addr));
            // Read read of bytes
            cmd_table.prdt_entry[prdt_length].set_byte_count(0xFFF);
            bytes_to_read -= 0x1000;
            ptr_addr += 0x1000;
            prdt_length += 1;
        }

        if bytes_to_read > 0 {
            let phys_addr = mapper.get_phys_addr(ptr_addr).unwrap();
            cmd_table.prdt_entry[prdt_length].set_data_base_address(Self::get_address(phys_addr));
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

        let mut spin = 0;

        while ((self.hba_port.task_file_data.read() & (0x80 | 0x08)) > 0) && spin < 1000000 {
            spin += 1;
        }
        if spin == 1000000 {
            println!("Port is hung");
            return None;
        }

        self.hba_port.command_issue.write(1 << slot);
        loop {
            yield_now();
            println!("Test");
            if self.hba_port.command_issue.read() & (1 << slot) == 0 {
                break;
            }
            if self.hba_port.interrupt_status.read() & (1 << 30) > 0 {
                println!("Err");
                return None;
                // Read error
            }
        }
        if self.hba_port.interrupt_status.read() & (1 << 30) > 0 {
            println!("Err");
            return None; // Read error
        }

        Some(())
    }
}
