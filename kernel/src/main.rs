#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate kernel;

use core::ffi::c_void;
use core::mem::transmute;
use core::ptr::slice_from_raw_parts_mut;

use ::acpi::{AcpiError, RsdpError};
use acpi::sdt::Signature;
use bootloader::{entry_point, BootInfo};
use kernel::boot_aps::boot_aps;
use kernel::cpu_localstorage::get_current_cpu_id;
use kernel::hpet::init_hpet;
use kernel::interrupts::{self};

use kernel::ioapic::{enable_apic, Madt};
use kernel::lapic::enable_localapic;
use kernel::memory::MemoryMapIter;
use kernel::net::ethernet::{ethernet_task, lookup_ip};
use kernel::paging::identity_map::{create_full_identity_map, FULL_IDENTITY_MAP};
use kernel::paging::page_allocator::{free_page, request_page};
use kernel::pci::enumerate_pci;
use kernel::pit::set_divisor;
use kernel::ps2::PS2Controller;
use kernel::scheduling::taskmanager::core_start_multitasking;
use kernel::screen::gop::{self, WRITER};
use kernel::screen::psf1;
use kernel::syscall::{sleep, spawn_process, spawn_thread, yield_now};
use kernel::uefi::get_config_table;
use kernel::{allocator, gdt, paging, BOOT_INFO};

use uefi::table::cfg::ACPI2_GUID;
use uefi::table::{Runtime, SystemTable};

// #[no_mangle]
entry_point!(main);

pub fn main(info: *const BootInfo) -> ! {
    unsafe {
        BOOT_INFO = transmute(info);
    }
    let boot_info = unsafe { core::ptr::read(info) };

    let font = psf1::load_psf1_font(boot_info.font);

    gop::WRITER.lock().set_gop(boot_info.gop, font);
    // Test screen colours
    // gop::WRITER.lock().fill_screen(0xFF_00_00);
    // gop::WRITER.lock().fill_screen(0x00_FF_00);
    // gop::WRITER.lock().fill_screen(0x00_00_FF);
    log!("Fill screen");
    gop::WRITER.lock().fill_screen(0);
    log!("Welcome to Fioxa...");

    log!("Getting UEFI runtime table");
    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    log!("Disabling interrupts...");
    x86_64::instructions::interrupts::disable();

    // Init the frame allocator
    log!("Initializing Frame Allocator...");

    let mmap_buf = unsafe {
        &*slice_from_raw_parts_mut(
            boot_info.mmap_buf,
            boot_info.mmap_len * boot_info.mmap_entry_size,
        )
    };
    let mmap = MemoryMapIter::new(mmap_buf, boot_info.mmap_entry_size, boot_info.mmap_len);

    unsafe { paging::page_allocator::init(mmap.clone()) };

    log!("Initializing GDT...");
    gdt::init(0);

    log!("Initalizing IDT...");
    interrupts::init_idt();

    create_full_identity_map(mmap);

    FULL_IDENTITY_MAP.lock().load_into_cr3();

    unsafe {
        let frame = request_page().unwrap();
        FULL_IDENTITY_MAP
            .lock()
            .map_memory(0x600000000, frame as u64, false)
            .unwrap()
            .flush();
        let f = frame as *mut u64;

        *f = 4493;
        assert!(
            *((0x600000000 as u64) as *const u64) == 4493,
            "Paging test failed"
        );
        FULL_IDENTITY_MAP
            .lock()
            .unmap_memory(0x600000000)
            .unwrap()
            .flush();
        free_page(frame);
    }

    log!("Initializing HEAP...");
    allocator::init_heap(&mut FULL_IDENTITY_MAP.lock()).expect("Heap initialization failed");

    // Set unicode mapping buffer (for more chacters than ascii)
    WRITER.lock().generate_unicode_mapping(font.unicode_buffer);

    let config_tables = runtime_table.config_table();

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .and_then(|acpi2_table| kernel::acpi::prepare_acpi(acpi2_table.address as usize))
        .unwrap();

    // Set PIT timer frequency
    set_divisor(65535);

    init_hpet(&acpi_tables);

    let madt = unsafe { acpi_tables.get_sdt::<Madt>(Signature::MADT) }
        .unwrap()
        .unwrap();

    enable_localapic(&mut FULL_IDENTITY_MAP.lock());

    enable_apic(&madt, &mut FULL_IDENTITY_MAP.lock());

    boot_aps(&madt);
    spawn_process(after_boot);

    unsafe { core_start_multitasking() };
}

fn after_boot() {
    let boot_info = unsafe { &*BOOT_INFO };

    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    let config_tables = runtime_table.config_table();

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .unwrap();

    let acpi_tables = kernel::acpi::prepare_acpi(acpi_tables.address as usize).unwrap();

    spawn_thread(|| {
        log!("Initalizing PS2 devices...");
        let mut ps2_controller = PS2Controller::new();

        if let Err(e) = ps2_controller.initialize() {
            log!("PS2 Controller failed to init because: {}", e);
            return;
        }
        loop {
            ps2_controller.check_packets();
            yield_now();
        }
    });

    spawn_thread(move || {
        log!("Enumnerating PCI...");

        enumerate_pci(acpi_tables);

        for i in 0..255 {
            let ip = kernel::net::ethernet::IPAddr::V4(192, 168, 1, i);
            println!(
                "IP: {:?} has MAC: {:#X}",
                &ip,
                lookup_ip(ip.clone()).unwrap_or(u64::MAX >> 4)
            );
        }
    });

    spawn_thread(|| {
        for i in 0.. {
            println!("Core: {}", get_current_cpu_id());
            println!("Uptime: {i}s");
            sleep(1000);
        }
    });
    spawn_thread(ethernet_task);
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    log!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
