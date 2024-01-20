#![no_std]
#![no_main]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[macro_use]
extern crate kernel;

use core::ffi::c_void;

use ::acpi::{AcpiError, RsdpError};
use acpi::sdt::Signature;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use kernel::boot_aps::boot_aps;
use kernel::bootfs::{DEFAULT_FONT, PS2_DRIVER, TERMINAL_ELF};
use kernel::cpu_localstorage::init_bsp_task;
use kernel::elf::load_elf;
use kernel::fs::{self, FSDRIVES};
use kernel::interrupts::{self, check_interrupts};

use kernel::ioapic::{enable_apic, Madt};
use kernel::lapic::{enable_localapic, map_lapic};
use kernel::memory::MemoryMapIter;
use kernel::net::ethernet::userspace_networking_main;
use kernel::paging::offset_map::{create_kernel_map, create_offset_map};
use kernel::paging::page_mapper::PageMapping;
use kernel::paging::page_table_manager::{ensure_ident_map_curr_process, Mapper, Page, Size4KB};
use kernel::paging::{
    get_uefi_active_mapper, set_mem_offset, virt_addr_offset, MemoryLoc, MemoryMappingFlags,
};
use kernel::pci::enumerate_pci;
use kernel::scheduling::process::Process;
use kernel::scheduling::taskmanager::{core_start_multitasking, PROCESSES};
use kernel::screen::gop::{self, Writer};
use kernel::screen::psf1;
use kernel::service::{ServiceInfo, SERVICES};
use kernel::time::pit::start_switching_tasks;
use kernel::time::{init_time, sleepd};
use kernel::uefi::get_config_table;
use kernel::{elf, gdt, paging, service, BOOT_INFO};

use bootloader::uefi::table::cfg::ACPI2_GUID;
use bootloader::uefi::table::{Runtime, SystemTable};

use kernel_userspace::ids::{ProcessID, ServiceID};
use kernel_userspace::service::register_public_service;
use kernel_userspace::syscall::{
    exit, receive_service_message_blocking, service_create, spawn_process, spawn_thread,
};

// #[no_mangle]
entry_point!(main);

pub fn main(info: *const BootInfo) -> ! {
    let mmap = unsafe {
        x86_64::instructions::interrupts::disable();

        // init gdt & idt
        gdt::init_bootgdt();
        interrupts::init_idt();

        BOOT_INFO = info;
        let boot_info = info.read();
        // get memory map
        let mmap = MemoryMapIter::new(
            boot_info.mmap_buf,
            boot_info.mmap_entry_size,
            boot_info.mmap_len,
        );

        // Initialize page allocator
        paging::page_allocator::init(mmap.clone());

        // Initalize GOP stdout
        let font = psf1::load_psf1_font(DEFAULT_FONT).expect("cannot load psf1 font");
        gop::WRITER.init_once(|| Writer::new(boot_info.gop, font).into());
        // Test screen colours
        gop::WRITER.get().unwrap().lock().fill_screen(0xFF_00_00);
        gop::WRITER.get().unwrap().lock().fill_screen(0x00_FF_00);
        gop::WRITER.get().unwrap().lock().fill_screen(0x00_00_FF);
        gop::WRITER.get().unwrap().lock().fill_screen(0xFF_FF_FF);
        gop::WRITER.get().unwrap().lock().fill_screen(0x00_00_00);

        mmap
    };

    log!("Welcome to Fioxa...");

    let init_process = Process::new(kernel::scheduling::process::ProcessPrivilige::KERNEL, &[]);
    assert!(init_process.pid == ProcessID(0));

    PROCESSES
        .lock()
        .insert(init_process.pid, init_process.clone());

    // remap and jump kernel to correct location
    unsafe {
        let map_addr = {
            let mut mem = init_process.memory.lock();

            // we need to set 0x8000 for the trampoline
            mem.page_mapper.insert_mapping_at(
                0x8000,
                PageMapping::new_mmap(0x8000, 0x1000),
                MemoryMappingFlags::WRITEABLE,
            );

            let map = mem.page_mapper.get_mapper_mut();

            create_offset_map(
                &mut map.get_next_table(Page::<Size4KB>::new(MemoryLoc::PhysMapOffset as u64)),
                mmap,
            );
            // get boot_info
            let boot_info = &*info;

            create_kernel_map(
                &mut map.get_next_table(Page::<Size4KB>::new(MemoryLoc::KernelStart as u64)),
                boot_info,
            );

            println!("Remapping to higher half");
            map.shift_table_to_offset();
            set_mem_offset(MemoryLoc::PhysMapOffset as u64);
            map.into_page().get_address()
        };
        // load and jump stack
        core::arch::asm!(
            "add rsp, {}",
            "mov cr3, {}",
            in(reg) MemoryLoc::PhysMapOffset as u64,
            in(reg) map_addr,
        );
    }

    let info = virt_addr_offset(info);
    // set global boot info
    unsafe { BOOT_INFO = info };

    // read boot_info
    let boot_info = unsafe { core::ptr::read(info) };

    log!("Initializing BSP for multicore...");
    unsafe { init_bsp_task() };

    unsafe {
        get_uefi_active_mapper()
            .identity_map_memory(
                Page::<Size4KB>::containing(boot_info.uefi_runtime_table),
                MemoryMappingFlags::empty(),
            )
            .unwrap()
            .ignore();
    }
    log!("Getting UEFI runtime table");
    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    log!("Loading UEFI runtime table");
    let config_tables = runtime_table.config_table();

    println!("Config table: ptr{:?}", config_tables.as_ptr());

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .and_then(|acpi2_table| kernel::acpi::prepare_acpi(acpi2_table.address as usize))
        .unwrap();

    init_time(&acpi_tables);

    let madt = unsafe { acpi_tables.get_sdt::<Madt>(Signature::MADT) }
        .unwrap()
        .unwrap();

    unsafe {
        map_lapic(&mut init_process.memory.lock().page_mapper.get_mapper_mut());
    }
    enable_localapic();

    unsafe { core::arch::asm!("sti") };

    unsafe {
        enable_apic(
            &madt,
            &mut init_process.memory.lock().page_mapper.get_mapper_mut(),
        );
    }

    unsafe { boot_aps(&madt) };
    spawn_process(after_boot, &[], true);
    spawn_process(check_interrupts, &[], true);
    spawn_process(sleepd, &[], true);

    // Disable interrupts so when we enable switching this core can finish init.
    unsafe { core::arch::asm!("cli") };
    start_switching_tasks();

    println!("Start multi");

    unsafe { core_start_multitasking() };
}

fn after_boot() {
    println!("After boot");

    // TODO: Reclaim memory, but first need to drop any references to the memory region
    // unsafe {
    //     let reclaim = frame_alloc_exec(|f| f.reclaim_memory());
    //     println!("RECLAIMED MEMORY: {}Mb", reclaim * 0x1000 / 1024 / 1024);
    // }

    let boot_info = unsafe { &*BOOT_INFO };

    unsafe {
        ensure_ident_map_curr_process(
            Page::<Size4KB>::containing(boot_info.uefi_runtime_table),
            MemoryMappingFlags::empty(),
        )
    };

    let runtime_table =
        unsafe { SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void) }
            .unwrap();

    let config_tables = runtime_table.config_table();

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::Rsdp(RsdpError::NoValidRsdp))
        .unwrap();

    let acpi_tables = kernel::acpi::prepare_acpi(acpi_tables.address as usize).unwrap();

    let mgmt_pid = spawn_process(service::start_mgmt, &[], true);

    // Insert mgmt service to prevent race condition of process spawn
    SERVICES.lock().insert(
        ServiceID(1),
        ServiceInfo {
            owner: mgmt_pid,
            subscribers: Default::default(),
        },
    );

    spawn_process(elf::elf_new_process_loader, &[], true);

    spawn_process(gop::gop_entry, &[], true);
    spawn_thread(fs::file_handler);

    log!("Enumnerating PCI...");

    enumerate_pci(acpi_tables);

    spawn_process(userspace_networking_main, &[], true);

    spawn_thread(|| FSDRIVES.lock().identify());

    // TODO: Use IO permissions instead of kernel
    load_elf(PS2_DRIVER, &[], true).unwrap();
    load_elf(TERMINAL_ELF, &[], false).unwrap();

    // For testing, accepts all inputs
    spawn_process(
        || {
            let sid = service_create();
            register_public_service("ACCEPTER", sid, &mut Vec::new());

            for i in 0.. {
                receive_service_message_blocking(sid);
                if i % 10000 == 0 {
                    println!("ACCEPTER: {i}")
                }
            }
        },
        &[],
        true,
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
