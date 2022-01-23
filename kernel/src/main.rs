#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate kernel;

use ::acpi::{AcpiError, RsdpError};
use bootloader::{entry_point, BootInfo};
use kernel::{acpi, allocator, interrupts};

use kernel::{gdt, screen::gop};

use kernel::{
    memory::uefi::{identity_map_all_memory, FRAME_ALLOCATOR},
    pci::enumerate_pci,
    pit::{set_frequency, start_switching_tasks},
    ps2::PS2Controller,
    syscall::{sleep, spawn_thread, yield_now},
};

// #[no_mangle]
entry_point!(main);
pub fn main(info: *const BootInfo<'static>) -> ! {
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
    FRAME_ALLOCATOR.lock().init(&boot_info.mmap.clone());

    log!("Initalizing Frame Mapper...");
    // Remap the memory and get a mapper
    let mut mapper = unsafe { identity_map_all_memory(&boot_info.mmap.clone()) };

    log!("Initializing HEAP...");
    allocator::init_heap(&mut mapper).expect("Heap initialization failed");

    // Convert option to AcpiError
    let rsdp = boot_info
        .rsdp_address
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp));

    // Get the ACPI table from the RSDP
    let acpi_tables = rsdp.and_then(|r| acpi::prepare_acpi(r));

    // // Enumerate PCI
    log!("Enumnerating PCI...");
    enumerate_pci(acpi_tables);

    // Unicode Mapping enable
    gop::WRITER
        .lock()
        .generate_unicode_mapping(boot_info.font.unicode_buffer);

    // 100 Times a seccond
    // About every 10ms
    set_frequency(100);

    log!("Enabling interrupts");
    x86_64::instructions::interrupts::enable();

    log!("Initalizing PS2 devices...");
    let mut ps2_controller = PS2Controller::new();
    if let Err(e) = ps2_controller.initialize() {
        log!("PS2 Controller failed to init because: {}", e)
    }

    spawn_thread(|| loop {
        // Check for new ps2 packets
        ps2_controller.check_packets();

        yield_now();
    });

    spawn_thread(|| {
        for i in 0..60 {
            println!("{}", i);
            sleep(1000);
        }
    });

    spawn_thread(|| loop {
        sleep(1000 * 60);
        println!("1 Minute");
    });

    log!("Begin task manager");

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
