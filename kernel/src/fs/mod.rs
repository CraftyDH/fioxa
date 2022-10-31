use alloc::{boxed::Box, vec::Vec};
use spin::Mutex;

use crate::driver::disk::DiskBusDriver;

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
}
