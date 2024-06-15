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
#![feature(new_uninit)]
#![feature(asm_const)]
#![feature(error_in_core)]
#![feature(const_try)]

use core::fmt::Write;

use bootloader::BootInfo;
use kernel_userspace::syscall::exit;
use scheduling::taskmanager::kill_bad_task;
use screen::gop::WRITER;
use terminal::Writer;

use crate::{
    cpu_localstorage::CPULocalStorageRW, paging::MemoryLoc, scheduling::without_context_switch,
};

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

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
pub mod event;
pub mod fs;
pub mod gdt;
pub mod interrupts;
pub mod ioapic;
pub mod lapic;
pub mod locked_mutex;
pub mod logging;
pub mod memory;
pub mod message;
pub mod net;
pub mod paging;
pub mod pci;
pub mod scheduling;
pub mod socket;
pub mod syscall;
pub mod terminal;
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
    let context = CPULocalStorageRW::get_context();
    if context == 0 {
        // lowest context, no chance of recovery
        without_context_switch(|| {
            let mut w = WRITER.get().unwrap().lock();
            w.write_fmt(format_args!(
                "KERNEL PANIC: {}\n  Caused by {:?}, {:?}\n",
                info,
                CPULocalStorageRW::get_current_pid(),
                CPULocalStorageRW::get_current_tid()
            ))
            .unwrap();
            // since we drop context switch manually trigger redraw
            w.redraw_if_needed();
            crate::stack_trace(&mut w);
            w.redraw_if_needed();
            loop {
                unsafe { core::arch::asm!("hlt") }
            }
        })
    } else if context == 1 {
        // see if we can recover
        error!("KERNEL PANIC: {}", info);
        kill_bad_task()
    } else {
        // it's a kernel thread so exit
        error!(
            "KERNEL THREAD PANIC: Caused by {:?} {:?}\n{}",
            CPULocalStorageRW::get_current_pid(),
            CPULocalStorageRW::get_current_tid(),
            info
        );
        exit()
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation Error: {:?}", layout)
}

/// Walks rbp to find all call frames, additionally prints out the return address of each frame
/// TODO: find the associated function from the ip
pub fn stack_trace(w: &mut Writer) {
    unsafe {
        let mut rbp: usize;
        w.write_str("Performing stack trace...\n").unwrap();
        core::arch::asm!("mov {}, rbp", lateout(reg) rbp);
        for depth in 0.. {
            let caller = *((rbp + 8) as *const usize);
            w.write_fmt(format_args!(
                "Frame {depth}: base pointer: {rbp:#x}, return address: {caller:#x}\n"
            ))
            .unwrap();

            rbp = *(rbp as *const usize);
            // at rbp 0 we have walked to the end
            if rbp == 0 {
                w.write_str("Stack trace finished.\n").unwrap();
                return;
            } else if rbp <= MemoryLoc::EndUserMem as usize {
                w.write_fmt(format_args!(
                    "Stopping at user mode, base pointer: {rbp:#x}\n"
                ))
                .unwrap();
                return;
            }
        }
    }
}
