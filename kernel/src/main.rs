#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)] // We need to be able to create the error handler
#![feature(const_mut_refs)]
#![feature(asm)]
//* IDK, BUT the wrapper function needs it */
#![feature(asm_sym)]
#![feature(naked_functions)]
#![feature(fn_traits)]
//* */
#![macro_use]
extern crate alloc;

#[macro_use]
mod screen;
mod acpi;
mod allocator;
mod assembly;
mod gdt;
mod interrupts;
mod locked_mutex;
mod memory;
mod multitasking;
mod pci;
mod pit;
mod ps2;
mod syscall;

use core::panic::PanicInfo;

use screen::gop;
use types::BootInfo;

use crate::{
    memory::uefi::{identity_map_all_memory, FRAME_ALLOCATOR},
    pci::enumerate_pci,
    pit::{set_frequency, sleep, start_switching_tasks},
    ps2::PS2Controller,
    screen::gop::WRITER,
    syscall::{spawn_thread, yield_now},
};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { WRITER.force_unlock() };
    println!("Panic: {}", info);
    loop {}
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation Error: {:?}", layout)
}

#[no_mangle]
// entry_point!(main);
pub extern "C" fn _start(info: *const BootInfo<'static>) -> ! {
    let boot_info = unsafe { core::ptr::read(info) };
    gop::WRITER.lock().set_gop(boot_info.gop, boot_info.font);
    // Test screen colours
    // gop::WRITER.lock().fill_screen(0xFF_00_00);
    // gop::WRITER.lock().fill_screen(0x00_FF_00);
    // gop::WRITER.lock().fill_screen(0x00_00_FF);
    gop::WRITER.lock().fill_screen(0);
    println!("Welcome to Fioxa...");

    println!("Disabling interrupts...");
    x86_64::instructions::interrupts::disable();

    println!("Initializing GDT...");
    gdt::init();

    println!("Initalizing IDT...");
    interrupts::init_idt();

    // Init the frame allocator
    println!("Initializing Frame Allocator...");
    FRAME_ALLOCATOR.lock().init(&boot_info.mmap.clone());

    println!("Initalizing Frame Mapper...");
    // Remap the memory and get a mapper
    let mut mapper = unsafe { identity_map_all_memory(&boot_info.mmap.clone()) };

    println!("Initializing HEAP...");
    allocator::init_heap(&mut mapper).expect("Heap initialization failed");

    let acpi_tables = acpi::prepare_acpi(boot_info.rsdp).unwrap();

    // Enumerate PCI
    println!("Enumnerate PCI");
    enumerate_pci(&acpi_tables);

    // Unicode Mapping enable
    gop::WRITER
        .lock()
        .generate_unicode_mapping(boot_info.font.unicode_buffer);

    // 100 Times a seccond
    // About every 10ms
    set_frequency(100);

    println!("Enabling interrupts");
    x86_64::instructions::interrupts::enable();

    println!("Initalizing PS2 devices...");
    let mut ps2_controller = PS2Controller::new();
    if let Err(e) = ps2_controller.initialize() {
        println!("PS2 Controller failed to init because: {}", e)
    }
    // TASKMANAGER.lock().init(mapper);

    spawn_thread(|| loop {
        // Check for new ps2 packets
        ps2_controller.check_packets();

        // Wake up slept tasks
        // sleep_waker();

        yield_now();
    });

    // spawn_thread(|| {
    //     for i in 0..50 {
    //         println!("{}", i);
    //         sleep(1000);
    //     }
    // });

    // spawn_thread(|| loop {
    //     print!(".");
    //     yield_now()
    // });

    // spawn_thread(|| loop {
    //     println!("1 Minute");
    //     sleep(1000 * 60);
    // });

    // spawn_thread(|| loop {
    //     println!("1/2");
    //     sleep(ms);
    // });

    // for i in 0..5 {
    //     print!("{} ", get_uptime() / 1000);
    //     sleep(1000);
    // }

    // set_frequency(1);

    println!("Begin task manager");
    start_switching_tasks();

    // Wait a tick for the timer interrupt to trigger the multitasking
    loop {}
}

// #[derive(Clone)]
// pub struct FioxaAcpiHandler;

// impl acpi::AcpiHandler for FioxaAcpiHandler {
//     unsafe fn map_physical_region<T>(
//         &self,
//         physical_address: usize,
//         size: usize,
//     ) -> acpi::PhysicalMapping<Self, T> {
//         PhysicalMapping::new(
//             physical_address,
//             NonNull::new(physical_address as *mut T).unwrap(),
//             size,
//             size,
//             Self,
//         )
//     }

//     fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
// }
