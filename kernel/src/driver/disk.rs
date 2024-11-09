pub mod ahci;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_userspace::disk::ata::ATADiskIdentify;

use crate::mutex::Spinlock;

use super::driver::Driver;

pub trait DiskBusDriver: Driver {
    fn get_disks(&mut self) -> Vec<Arc<Spinlock<dyn DiskDevice>>>;
    fn get_disk_by_id(&mut self, id: usize) -> Option<Arc<Spinlock<dyn DiskDevice>>>;
}

pub trait DiskDevice: Send + Sync {
    fn read(&mut self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()>;
    fn write(&mut self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()>;
    fn identify(&mut self) -> Box<ATADiskIdentify>;
}
