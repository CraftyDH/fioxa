#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)] // We need to be able to create the error handler
#![feature(const_mut_refs)]
//* IDK, BUT the wrapper function needs it */
#![feature(asm_sym)]
#![feature(naked_functions)]
#![feature(fn_traits)]
//* Testing
// #![feature(custom_test_frameworks)]
// #![test_runner(test_runner)]
#![feature(panic_info_message)]

//* */
#[macro_use]
extern crate alloc;

#[macro_use]
pub mod screen;
pub mod acpi;
pub mod allocator;
pub mod assembly;
pub mod gdt;
pub mod interrupts;
pub mod locked_mutex;
pub mod memory;
pub mod multitasking;
pub mod paging;
pub mod pci;
pub mod pit;
pub mod ps2;
pub mod syscall;
pub mod uefi;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log!("Panic: {}", info);
    loop {}
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
