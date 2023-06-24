#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)] // We need to be able to create the error handler
#![feature(const_mut_refs)]
#![feature(naked_functions)]
#![feature(fn_traits)]
//* Testing
// #![feature(custom_test_frameworks)]
// #![test_runner(test_runner)]
#![feature(panic_info_message)]
#![feature(const_for)]
#![feature(pointer_byte_offsets)]
#![feature(new_uninit)]

use bootloader::BootInfo;

use crate::scheduling::without_context_switch;

#[macro_use]
extern crate alloc;

#[macro_use]
pub mod screen;
pub mod acpi;
pub mod allocator;
pub mod assembly;
pub mod boot_aps;
pub mod bootfs;
pub mod cpu_localstorage;
pub mod driver;
pub mod elf;
pub mod fs;
pub mod gdt;
pub mod interrupts;
pub mod ioapic;
pub mod lapic;
pub mod locked_mutex;
pub mod memory;
pub mod net;
pub mod paging;
pub mod pci;
pub mod ps2;
pub mod scheduling;
pub mod service;
pub mod syscall;
pub mod time;
pub mod uefi;

pub static mut BOOT_INFO: *const BootInfo = 0 as *const BootInfo;
extern "C" {
    static KERNEL_START: u8;
    static KERNEL_END: u8;
}

pub fn kernel_memory_loc() -> (u64, u64) {
    // Safe since these are our own linker variables
    unsafe {
        (
            &KERNEL_START as *const u8 as u64,
            &KERNEL_END as *const u8 as u64,
        )
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // unsafe { WRITER.force_unlock() };
    // unsafe { core::arch::asm!("cli") }
    without_context_switch(|| {
        log!("Panic: {}", info);
        loop {}
    })
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation Error: {:?}", layout)
}

#[macro_export]
macro_rules! log {
    () => ({
        // s_print("\n");
        print!("\n");
    });
    ($($arg:tt)*) => ({
        // s_print!("{}\n", format_args!($($arg)*));
        print!("{}\n", format_args!($($arg)*));
    });
}
