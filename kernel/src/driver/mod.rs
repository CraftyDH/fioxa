use crate::pci::PCIHeaderCommon;

pub mod disk;

pub trait Driver: Send {
    fn new(device: PCIHeaderCommon) -> Option<Self>
    where
        Self: Sized;
    fn unload(self) -> !;
    // When a pci device sends an interrupt
    fn interrupt_handler(&mut self);
}
