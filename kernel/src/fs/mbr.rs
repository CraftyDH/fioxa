use alloc::sync::Arc;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{disk::DiskService, mutex::Mutex};

use crate::fs::{FSPartitionDisk, fat::read_bios_block};

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

pub fn read_partitions(drive: Arc<Mutex<DiskService>>) {
    let mbr = drive.lock().read(0, 1).deserialize().unwrap();
    let mbr = unsafe { &mut *(mbr.as_ptr() as *mut MasterBootRecord) };

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
                FSPartitionDisk::new(drive.clone(), part.start_lba as u64, part.length as u64);
            sys_process_spawn_thread(|| read_bios_block(fs_disk));
        }
    }
}
