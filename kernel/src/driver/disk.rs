pub mod ahci;

use super::driver::Driver;

pub trait DiskBusDriver: Driver {
    fn read(&mut self, dev: u8, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()>;
    fn write(
        &mut self,
        dev: usize,
        sector: usize,
        sector_count: u32,
        buffer: &mut [u8],
    ) -> Option<()>;
}
