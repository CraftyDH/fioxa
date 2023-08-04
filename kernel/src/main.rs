#![no_std]
#![no_main]
#![feature(pointer_byte_offsets)]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[macro_use]
extern crate kernel;

use core::ffi::c_void;
use core::mem::{size_of, transmute};
use core::ptr::slice_from_raw_parts_mut;

use ::acpi::{AcpiError, RsdpError};
use acpi::sdt::Signature;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use kernel::boot_aps::boot_aps;
use kernel::bootfs::TERMINAL_ELF;
use kernel::cpu_localstorage::init_bsp_task;
use kernel::fs::{self, FSDRIVES};
use kernel::interrupts::{self};

use kernel::ioapic::{enable_apic, Madt};
use kernel::lapic::enable_localapic;
use kernel::memory::MemoryMapIter;
use kernel::net::ethernet::userspace_networking_main;
use kernel::paging::offset_map::{create_kernel_map, create_offset_map, map_gop};
use kernel::paging::page_allocator::{frame_alloc_exec, request_page};
use kernel::paging::page_table_manager::{Mapper, Page, Size4KB};
use kernel::paging::{
    get_uefi_active_mapper, set_mem_offset, virt_addr_for_phys, MemoryLoc, KERNEL_MAP,
};
use kernel::pci::enumerate_pci;
use kernel::scheduling::taskmanager::core_start_multitasking;
use kernel::screen::gop::{self, WRITER};
use kernel::screen::psf1::{self, load_psf1_font};
use kernel::time::init_time;
use kernel::time::pit::start_switching_tasks;
use kernel::uefi::get_config_table;
use kernel::{elf, gdt, paging, ps2, service, BOOT_INFO};

use bootloader::uefi::table::cfg::{ConfigTableEntry, ACPI2_GUID};
use bootloader::uefi::table::{Runtime, SystemTable};
use kernel_userspace::service::{
    generate_tracking_number, get_public_service_id, register_public_service,
    SendServiceMessageDest, ServiceMessage,
};
use kernel_userspace::syscall::{
    exit, get_pid, receive_service_message_blocking, send_service_message, service_create,
    spawn_process, spawn_thread, yield_now,
};

// #[no_mangle]
entry_point!(main);

pub fn main(info: *const BootInfo) -> ! {
    let rsp: usize;

    unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp) }
    unsafe {
        BOOT_INFO = transmute(info);
    }

    let boot_info = unsafe { core::ptr::read(info) };

    let Ok(font) = psf1::load_psf1_font(boot_info.font) else {
        // We can't do anything because without a font printing is futile
        loop {}
    };

    gop::WRITER.lock().set_gop(boot_info.gop, font);
    // Test screen colours
    // gop::WRITER.lock().fill_screen(0xFF_00_00);
    // gop::WRITER.lock().fill_screen(0x00_FF_00);
    // gop::WRITER.lock().fill_screen(0x00_00_FF);
    log!("Fill screen");
    gop::WRITER.lock().fill_screen(0);
    log!("Welcome to Fioxa...");

    log!("Disabling interrupts...");
    x86_64::instructions::interrupts::disable();

    log!("Getting UEFI runtime table");
    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

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
    log!("Initalizing BOOT GDT...");
    unsafe { gdt::init_bootgdt() };

    log!("Initalizing IDT...");
    interrupts::init_idt();

    // {
    //     println!("{:?}", get_chunked_page_range(0, 0x1000));
    //     println!("{:?}", get_chunked_page_range(0, 0x20000));
    //     println!("{:?}", get_chunked_page_range(0, 0x20000 - 0x1000));
    //     println!("{:?}", get_chunked_page_range(0x2000, 0x400000));
    // }

    {
        let mut map = KERNEL_MAP.lock();

        // Remap this threads stack
        for page in ((rsp & !0xFFF) as u64..(rsp + 1024 * 1024 * 5) as u64).step_by(0x1000) {
            map.identity_map_memory(Page::<Size4KB>::new(page))
                .unwrap()
                .ignore();
        }

        // create_offset_map(&mut map.get_lvl3(0), mmap.clone());
        create_offset_map(
            &mut map.get_next_table(Page::<Size4KB>::new(MemoryLoc::PhysMapOffset as u64)),
            mmap,
        );
        create_kernel_map(
            &mut map.get_next_table(Page::<Size4KB>::new(MemoryLoc::KernelStart as u64)),
        );
        map_gop(&mut map);

        map.identity_map_memory(Page::<Size4KB>::containing(info as u64))
            .unwrap()
            .ignore();

        map.identity_map_memory(Page::<Size4KB>::containing(boot_info.uefi_runtime_table))
            .unwrap()
            .ignore();

        println!("Remapping to higher half");
        unsafe { set_mem_offset(MemoryLoc::PhysMapOffset as u64) }

        unsafe {
            frame_alloc_exec(|f| {
                Some({
                    f.push_up_to_offset_mapping();
                })
            });

            // load and jump stack
            core::arch::asm!(
                "add rsp, {}",
                "mov cr3, {}",
                in(reg) MemoryLoc::PhysMapOffset as u64,
                in(reg) map.get_lvl4_addr(),
            );
            map.shift_table_to_offset();
        }
    }

    println!("Paging enabled");

    unsafe {
        let frame = request_page().unwrap();
        let page = Page::new(0x400000000000);
        KERNEL_MAP.lock().map_memory(page, *frame).unwrap().flush();
        let f = virt_addr_for_phys(frame.get_address()) as *mut u64;

        println!("Page test");
        *f = 4493;
        assert!(
            *((0x400000000000 as u64) as *const u64) == 4493,
            "Paging test failed"
        );
        KERNEL_MAP.lock().unmap_memory(page).unwrap().flush();
    }

    // log!("Initializing HEAP...");
    // allocator::init_heap(&mut KERNEL_MAP.lock()).expect("Heap initialization failed");

    log!("Updating font...");
    // Set unicode mapping buffer (for more chacters than ascii)
    // And update font to use new mapping
    WRITER
        .lock()
        .update_font(load_psf1_font(boot_info.font).unwrap());

    log!("Loading UEFI runtime table");
    let config_tables = runtime_table.config_table();

    let base = (config_tables.as_ptr() as u64) & !0xFFF;
    for page in (base..config_tables.as_ptr() as u64
        + size_of::<ConfigTableEntry>() as u64 * config_tables.len() as u64
        + 0xFFF)
        .step_by(0x1000)
    {
        KERNEL_MAP
            .lock()
            .identity_map_memory(Page::<Size4KB>::new(page))
            .unwrap()
            .ignore();
    }

    println!("Config table: ptr{:?}", config_tables.as_ptr());

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .and_then(|acpi2_table| kernel::acpi::prepare_acpi(acpi2_table.address as usize))
        .unwrap();

    init_time(&acpi_tables);

    let madt = unsafe { acpi_tables.get_sdt::<Madt>(Signature::MADT) }
        .unwrap()
        .unwrap();

    log!("Initializing BSP for multicore...");
    unsafe { init_bsp_task() };

    enable_localapic(&mut KERNEL_MAP.lock());

    unsafe { core::arch::asm!("sti") };

    enable_apic(&madt, &mut KERNEL_MAP.lock());

    boot_aps(&madt);
    spawn_process(after_boot, &[], true);

    // Disable interrupts so when we enable switching this core can finish init.
    unsafe { core::arch::asm!("cli") };
    start_switching_tasks();

    println!("Start multi");

    unsafe { core_start_multitasking() };
}

fn after_boot() {
    unsafe {
        map_gop(&mut get_uefi_active_mapper());
    }

    let boot_info = unsafe {
        &*((BOOT_INFO as *const u8).add(MemoryLoc::PhysMapOffset as usize) as *const BootInfo)
    };

    let mut map = unsafe { get_uefi_active_mapper() };

    map.identity_map_memory(Page::<Size4KB>::containing(boot_info.uefi_runtime_table))
        .unwrap()
        .ignore();

    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    let config_tables = runtime_table.config_table();

    let base = (config_tables.as_ptr() as u64) & !0xFFF;
    for page in (base..config_tables.as_ptr() as u64
        + size_of::<ConfigTableEntry>() as u64 * config_tables.len() as u64)
        .step_by(0x1000)
    {
        map.identity_map_memory(Page::<Size4KB>::new(page))
            .unwrap()
            .ignore();
    }

    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    let config_tables = runtime_table.config_table();

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .unwrap();

    let acpi_tables = kernel::acpi::prepare_acpi(acpi_tables.address as usize).unwrap();

    spawn_process(service::start_mgmt, &[], true);
    spawn_process(elf::elf_new_process_loader, &[], true);

    spawn_process(ps2::main, &[], true);
    spawn_process(gop::gop_entry, &[], true);
    spawn_thread(fs::file_handler);

    log!("Enumnerating PCI...");

    enumerate_pci(acpi_tables);

    spawn_process(userspace_networking_main, &[], true);

    spawn_thread(|| FSDRIVES.lock().identify());

    spawn_thread(|| {
        let mut buffer = Vec::new();
        let elf = get_public_service_id("ELF_LOADER", &mut buffer).unwrap();
        let pid = get_pid();

        send_service_message::<(&[u8], &[u8])>(
            &ServiceMessage {
                service_id: elf,
                sender_pid: pid,
                tracking_number: generate_tracking_number(),
                destination: SendServiceMessageDest::ToProvider,
                message: (TERMINAL_ELF, &[]),
            },
            &mut buffer,
        )
        .unwrap();
    });

    // For testing, accepts all inputs
    spawn_process(
        || {
            let sid = service_create();
            register_public_service("ACCEPTER", sid, &mut Vec::new());
            let mut buffer = Vec::new();

            for i in 0.. {
                let _: ServiceMessage<()> =
                    receive_service_message_blocking(sid, &mut buffer).unwrap();
                if i % 10000 == 0 {
                    println!("ACCEPTER: {i}")
                }
            }
        },
        &[],
        false,
    );

    // spawn_thread(|| {
    //     for i in 0.. {
    //         request_page();
    //         if i % 10000 == 0 {
    //             println!("PAGE: {i} {}mb", i * 0x1000 / 1024 / 1024)
    //         }
    //     }
    // });

    exit();
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    log!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
