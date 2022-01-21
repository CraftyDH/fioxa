use x86_64::{
    instructions::interrupts::without_interrupts, structures::idt::InterruptDescriptorTable,
};

pub mod exceptions;
pub mod hardware;

use lazy_static::lazy_static;

use crate::syscall;

lazy_static! {
    pub static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // Set idt table
        exceptions::set_exceptions_idt(&mut idt);
        hardware::set_hardware_idt(&mut idt);
        syscall::set_syscall_idt(&mut idt);

        idt
    };
}

pub fn init_idt() {
    without_interrupts(|| {
        IDT.load();
        unsafe {
            let mut pics = hardware::PICS.lock();
            // Initialize the PICS
            pics.initialize();

            // Start with an empty bitmask
            // Except allow cascade
            pics.write_masks(0b1111_1010, 255);
        };
    })
}
