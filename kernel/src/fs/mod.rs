pub mod fat;
pub mod mbr;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use spin::Mutex;

use crate::{
    driver::disk::{DiskBusDriver, DiskDevice},
    fs::mbr::read_partitions,
};

lazy_static::lazy_static! {
    pub static ref ROOTFS: Mutex<FileSystem> = Mutex::new(FileSystem { disks_buses: Default::default() });
}
pub struct FileSystem {
    disks_buses: Vec<Box<dyn DiskBusDriver>>,
}

impl FileSystem {
    pub fn add_device(&mut self, device: Box<dyn DiskBusDriver>) {
        self.disks_buses.push(device);
    }

    pub fn identify(&mut self) {
        for bus in &mut self.disks_buses {
            for disk in bus.get_disks() {
                println!("{:?}", disk.lock().identify());
                read_partitions(disk);
            }
        }
    }
}

pub struct FSPartitionDisk {
    backing_disk: Arc<Mutex<dyn DiskDevice>>,
    partition_offset: usize,
    partition_length: usize,
}

impl FSPartitionDisk {
    pub fn new(
        backing_disk: Arc<Mutex<dyn DiskDevice>>,
        partition_offset: usize,
        partition_length: usize,
    ) -> Self {
        Self {
            backing_disk,
            partition_offset,
            partition_length,
        }
    }

    fn read(&self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()> {
        assert!(sector + sector_count as usize <= self.partition_length);
        self.backing_disk
            .lock()
            .read(sector + self.partition_offset, sector_count, buffer)
    }
}
