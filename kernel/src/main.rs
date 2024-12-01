#![no_std]
#![no_main]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use core::ffi::c_void;

use ::acpi::AcpiError;
use bootloader::{entry_point, BootInfo};
use kernel::acpi::FioxaAcpiHandler;
use kernel::boot_aps::boot_aps;
use kernel::bootfs::{DEFAULT_FONT, PS2_DRIVER, TERMINAL_ELF};
use kernel::cpu_localstorage::{init_bsp_localstorage, CPULocalStorageRW};
use kernel::elf::load_elf;
use kernel::fs::{self, FSDRIVES};
use kernel::interrupts::{self, check_interrupts};

use kernel::ioapic::{enable_apic, Madt};
use kernel::lapic::{enable_localapic, map_lapic};
use kernel::logging::KERNEL_LOGGER;
use kernel::memory::MemoryMapIter;
use kernel::net::ethernet::userspace_networking_main;
use kernel::paging::offset_map::{create_kernel_map, create_offset_map, map_gop};
use kernel::paging::page::{Page, Size4KB};
use kernel::paging::page_allocator::global_allocator;
use kernel::paging::page_table::Mapper;
use kernel::paging::{
    ensure_ident_map_curr_process, set_mem_offset, virt_addr_offset, MemoryLoc, MemoryMappingFlags,
    KERNEL_DATA_MAP, KERNEL_LVL4, OFFSET_MAP,
};
use kernel::pci::enumerate_pci;
use kernel::scheduling::process::Process;
use kernel::scheduling::taskmanager::{core_start_multitasking, push_task_queue, PROCESSES};
use kernel::scheduling::with_held_interrupts;
use kernel::screen::gop;
use kernel::screen::psf1;
use kernel::syscall::syscall_kernel_handler;
use kernel::terminal::Writer;
use kernel::time::init_time;
use kernel::uefi::get_config_table;
use kernel::{elf, gdt, paging, BOOT_INFO};

use bootloader::uefi::table::cfg::ACPI2_GUID;
use bootloader::uefi::table::{Runtime, SystemTable};

use kernel_userspace::backoff_sleep;
use kernel_userspace::ids::ProcessID;
use kernel_userspace::object::KernelReference;
use kernel_userspace::socket::{socket_connect, SocketListenHandle, SocketRecieveResult};
use kernel_userspace::syscall::{exit, set_syscall_fn, spawn_process, spawn_thread};

// #[no_mangle]
entry_point!(main_stage1);

pub fn main_stage1(info: *const BootInfo) -> ! {
    unsafe {
        x86_64::instructions::interrupts::disable();

        // init gdt & idt
        gdt::init_bootgdt();
        interrupts::init_idt();

        set_syscall_fn(syscall_kernel_handler as u64);

        let boot_info = info.read();
        // get memory map
        let mmap = MemoryMapIter::new(
            boot_info.mmap_buf,
            boot_info.mmap_entry_size,
            boot_info.mmap_len,
        );

        // Initialize page allocator
        paging::page_allocator::init(mmap.clone());

        let alloc = global_allocator();

        // Initialize global page maps
        create_offset_map(alloc, &mut OFFSET_MAP.lock(), mmap);
        create_kernel_map(alloc, &mut KERNEL_DATA_MAP.lock(), &boot_info);

        // Initialize scheduler / global table
        let cr3 = {
            let mut table = KERNEL_LVL4.lock();
            map_gop(global_allocator(), &mut table, &boot_info.gop);

            table
                .identity_map(
                    alloc,
                    Page::<Size4KB>::new(0xfee00000),
                    MemoryMappingFlags::WRITEABLE,
                )
                .unwrap()
                .ignore();

            table.get_physical_address()
        };

        set_mem_offset(MemoryLoc::PhysMapOffset as u64);
        BOOT_INFO = virt_addr_offset(info);

        // load and jump stack
        core::arch::asm!(
            "mov rbp, 0",
            "add rsp, {}",
            "mov cr3, {}",
            "jmp {}",
            in(reg) MemoryLoc::PhysMapOffset as u64,
            in(reg) cr3,
            in(reg) main_stage2,
            options(noreturn)
        );
    };
}

/// Interrupts should be disabled before calling
unsafe extern "C" fn main_stage2() {
    let boot_info = unsafe { core::ptr::read(BOOT_INFO) };

    // Initalize GOP stdout
    let font = psf1::load_psf1_font(DEFAULT_FONT).expect("cannot load psf1 font");
    gop::WRITER.init_once(|| Writer::new(boot_info.gop, font).into());
    // Test screen colours
    gop::WRITER.get().unwrap().lock().reset_screen(0xFF_00_00);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_FF_00);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_00_FF);
    gop::WRITER.get().unwrap().lock().reset_screen(0xFF_FF_FF);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_00_00);

    log::set_logger(&KERNEL_LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    info!("Welcome to Fioxa...");

    init_bsp_localstorage();

    let init_process = Process::new(kernel::scheduling::process::ProcessPrivilige::KERNEL, &[]);
    assert!(init_process.pid == ProcessID(0));

    PROCESSES
        .lock()
        .insert(init_process.pid, init_process.clone());

    push_task_queue(init_process.new_thread(init as *const u64, 0).unwrap());

    core_start_multitasking();
}

extern "C" fn init() {
    with_held_interrupts(|| {
        // read boot_info
        let boot_info = unsafe { core::ptr::read(BOOT_INFO) };

        let init_process = unsafe { CPULocalStorageRW::get_current_task().process() };

        unsafe {
            ensure_ident_map_curr_process(
                Page::<Size4KB>::containing(boot_info.uefi_runtime_table),
                MemoryMappingFlags::empty(),
            );
        }
        info!("Getting UEFI runtime table");
        let runtime_table = unsafe {
            SystemTable::<Runtime>::from_ptr(boot_info.uefi_runtime_table as *mut c_void)
        }
        .unwrap();

        info!("Loading UEFI runtime table");
        let config_tables = runtime_table.config_table();

        info!("Config table: ptr{:?}", config_tables.as_ptr());

        unsafe {
            ensure_ident_map_curr_process(
                Page::<Size4KB>::containing(config_tables.as_ptr() as u64),
                MemoryMappingFlags::empty(),
            );
        }

        let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
            .ok_or(AcpiError::NoValidRsdp)
            .and_then(|acpi2_table| unsafe {
                acpi::AcpiTables::from_rsdp(FioxaAcpiHandler, acpi2_table.address as usize)
            })
            .unwrap();

        init_time(&acpi_tables);

        let madt = acpi_tables.find_table::<Madt>().unwrap();

        unsafe {
            map_lapic(&mut init_process.memory.lock().page_mapper.get_mapper_mut());
            enable_localapic();
        }

        unsafe {
            enable_apic(
                &madt,
                &mut init_process.memory.lock().page_mapper.get_mapper_mut(),
            );
        }

        unsafe { boot_aps(&madt) };
    });

    // TODO: Reclaim memory, but first need to drop any references to the memory region
    // unsafe {
    //     let reclaim = frame_alloc_exec(|f| f.reclaim_memory());
    //     println!("RECLAIMED MEMORY: {}Mb", reclaim * 0x1000 / 1024 / 1024);
    // }

    spawn_process(check_interrupts, &[], true);
    spawn_process(elf::elf_new_process_loader, &[], true);
    spawn_process(gop::gop_entry, &[], true);
    spawn_process(userspace_networking_main, &[], true);
    spawn_process(testing_proc, &[], true);
    spawn_process(after_boot_pci, &[], true);

    // TODO: Use IO permissions instead of kernel
    load_elf(
        PS2_DRIVER,
        &[],
        &[KernelReference::from_id(backoff_sleep(|| {
            socket_connect("STDOUT")
        }))],
        true,
    )
    .unwrap();
    load_elf(
        TERMINAL_ELF,
        &[],
        &[KernelReference::from_id(backoff_sleep(|| {
            socket_connect("STDOUT")
        }))],
        false,
    )
    .unwrap();

    exit();
}

/// For testing, accepts all inputs
fn testing_proc() {
    let sid = SocketListenHandle::listen("ACCEPTER").unwrap();

    loop {
        let handle = sid.blocking_accept();
        spawn_thread(move || {
            for i in 0usize.. {
                match handle.blocking_recv() {
                    Ok(_) => (),
                    Err(SocketRecieveResult::EOF) => {
                        info!("ACCEPTED {i}");
                        return;
                    }
                    Err(e) => panic!("{e:?}"),
                };
                if i % 10000 == 0 {
                    info!("ACCEPTER: {i}")
                }
            }
        });
    }
}

fn after_boot_pci() {
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

    unsafe {
        ensure_ident_map_curr_process(
            Page::<Size4KB>::containing(config_tables.as_ptr() as u64),
            MemoryMappingFlags::empty(),
        );
    }

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::NoValidRsdp)
        .unwrap();

    let acpi_tables = unsafe {
        acpi::AcpiTables::from_rsdp(FioxaAcpiHandler, acpi_tables.address as usize).unwrap()
    };

    info!("Enumnerating PCI...");

    enumerate_pci(acpi_tables);

    spawn_thread(fs::file_handler);
    FSDRIVES.lock().identify();

    exit();
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    log!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
