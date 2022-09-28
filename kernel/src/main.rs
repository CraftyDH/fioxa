#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate kernel;

use core::arch::asm;
use core::ptr::slice_from_raw_parts_mut;

use ::acpi::{AcpiError, RsdpError};
use bootloader::{entry_point, BootInfo};
use kernel::interrupts::{self};

use kernel::paging::identity_map::identity_map;
use kernel::paging::page_allocator::{request_page, GLOBAL_FRAME_ALLOCATOR};
use kernel::paging::page_table_manager::PageTableManager;
use kernel::pci::enumerate_pci;
use kernel::pit::{set_frequency, start_switching_tasks};
use kernel::ps2::PS2Controller;
use kernel::screen::gop::{self, WRITER};
use kernel::syscall::{exit, sleep, spawn_thread, yield_now};
use kernel::{allocator, gdt, paging};
use spin::mutex::Mutex;

// #[no_mangle]
entry_point!(main);
pub fn main(info: *const BootInfo) -> ! {
    let boot_info = unsafe { core::ptr::read(info) };

    gop::WRITER.lock().set_gop(boot_info.gop, boot_info.font);
    // Test screen colours
    // gop::WRITER.lock().fill_screen(0xFF_00_00);
    // gop::WRITER.lock().fill_screen(0x00_FF_00);
    // gop::WRITER.lock().fill_screen(0x00_00_FF);
    gop::WRITER.lock().fill_screen(0);
    log!("Welcome to Fioxa...");

    log!("Disabling interrupts...");
    x86_64::instructions::interrupts::disable();

    log!("Initializing GDT...");
    gdt::init();

    log!("Initalizing IDT...");
    interrupts::init_idt();

    // Init the frame allocator
    log!("Initializing Frame Allocator...");
    let mmap = unsafe { &*slice_from_raw_parts_mut(boot_info.mmap, boot_info.mmap_size) };

    GLOBAL_FRAME_ALLOCATOR.init_once(|| {
        let allocator = unsafe { paging::page_allocator::PageFrameAllocator::new(mmap) };
        Mutex::new(allocator)
    });

    let pml4_addr = request_page().unwrap();

    let mut page_table_mngr = PageTableManager::new(pml4_addr as u64);

    identity_map(&mut page_table_mngr, mmap);

    let frame = request_page().unwrap();

    page_table_mngr.map_memory(0x600000000, frame as u64);

    unsafe { asm!("mov cr3, {}", in(reg) pml4_addr, options(nostack, preserves_flags)) };

    unsafe {
        let frame = frame as *mut u64;

        *frame = 4493;
        println!("Paging test 1 {} = 4493", *frame);
        println!(
            "Paging test 2 {} = 4493",
            *((0x600000000 as u64) as *const u64)
        );
    }

    log!("Initializing HEAP...");
    allocator::init_heap(&mut page_table_mngr).expect("Heap initialization failed");

    // Set unicode mapping buffer (for more chacters than ascii)
    WRITER
        .lock()
        .generate_unicode_mapping(boot_info.font.unicode_buffer);

    log!("Enabling interrupts");
    x86_64::instructions::interrupts::enable();

    // Set PIC timer frequency
    set_frequency(100);

    spawn_thread(|| {
        log!("Initalizing PS2 devices...");
        let mut ps2_controller = PS2Controller::new();
        if let Err(e) = ps2_controller.initialize() {
            log!("PS2 Controller failed to init because: {}", e);
            exit();
        }
        loop {
            ps2_controller.check_packets();

            yield_now();
        }
    });

    spawn_thread(|| {
        log!("Enumnerating PCI...");

        // Convert option to AcpiError
        let rsdp = boot_info
            .rsdp_address
            .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp));
        let acpi_tables = rsdp.and_then(|r| kernel::acpi::prepare_acpi(r));
        enumerate_pci(acpi_tables);
    });

    spawn_thread(|| {
        for i in 0.. {
            println!("UPTIME: {i}s");
            sleep(1000);
        }
    });

    start_switching_tasks();

    // Wait a tick for the timer interrupt to trigger the multitasking
    loop {}
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    log!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
