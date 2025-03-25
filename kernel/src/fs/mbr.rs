use alloc::sync::Arc;

use crate::{
    driver::disk::DiskDevice,
    fs::{FSPartitionDisk, fat::read_bios_block},
    mutex::Spinlock,
};

#[repr(C, packed)]
pub struct PartitionTableEntry {
    bootable: u8,
    _cfs: [u8; 3],
    partition_id: u8,
    _end_cfs: [u8; 3],
    start_lba: u32,
    length: u32,
}

#[repr(C, packed)]
pub struct MasterBootRecord {
    bootloader: [u8; 440],
    signature: u32,
    _unused: u16,

    partitions: [PartitionTableEntry; 4],
    magic_number: [u8; 2],
}

const MBR_SIZE: usize = 512;

pub fn read_partitions(drive: Arc<Spinlock<dyn DiskDevice>>) {
    // Round up to nearest 512 bytes
    let mbr_buf = &mut [0u8; MBR_SIZE];
    drive.lock().read(0, 1, mbr_buf);

    let mbr = unsafe { &mut *(mbr_buf.as_ptr() as *mut MasterBootRecord) };

    assert!(
        { mbr.magic_number } == [0x55, 0xAA],
        "MBR Magic number not valid, was given: {:?}",
        { mbr.magic_number }
    );

    for part in &mbr.partitions {
        if part.start_lba > 0 || part.bootable > 0 {
            info!(
                "Partition id {}: start:{} size:{}mb, bootable:{}",
                part.partition_id,
                { part.start_lba },
                part.length / 1024 * 512 / 1024,
                { part.bootable } == 0x80
            );
            let fs_disk =
                FSPartitionDisk::new(drive.clone(), part.start_lba as usize, part.length as usize);
            read_bios_block(fs_disk);
        }
    }
}
