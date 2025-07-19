#![no_std]
#![no_main]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use ::acpi::AcpiError;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bootloader::uefi::table::{set_system_table, system_table_raw};
use bootloader::{BootInfo, entry_point};
use kernel::acpi::FioxaAcpiHandler;
use kernel::boot_aps::boot_aps;
use kernel::bootfs::{DEFAULT_FONT, PS2_DRIVER};
use kernel::console::run_console;
use kernel::cpu_localstorage::{CPULocalStorageRW, init_bsp_boot_ls, init_bsp_localstorage};
use kernel::elf::load_elf;
use kernel::fs::{disk_controller, file_system_partition_loader, fs_controller};
use kernel::interrupts::{self, check_interrupts};

use kernel::ioapic::{Madt, enable_apic};
use kernel::lapic::{enable_localapic, map_lapic};
use kernel::logging::KERNEL_LOGGER;
use kernel::memory::MemoryMapIter;
use kernel::mutex::Spinlock;
use kernel::net::ethernet::userspace_networking_main;
use kernel::object::init_handle_new_proc;
use kernel::paging::offset_map::{create_kernel_map, create_offset_map, map_gop};
use kernel::paging::page_allocator::global_allocator;
use kernel::paging::{
    KERNEL_DATA_MAP, KERNEL_LVL4, MemoryLoc, OFFSET_MAP, set_mem_offset, virt_addr_offset,
};
use kernel::pci::enumerate_pci;
use kernel::scheduling::process::{
    ProcessBuilder, ProcessMemory, ProcessPrivilege, ProcessReferences,
};
use kernel::scheduling::taskmanager::{core_start_multitasking, spawn_process};
use kernel::scheduling::with_held_interrupts;
use kernel::screen::gop;
use kernel::screen::psf1;
use kernel::serial::{COM_1, SERIAL, Serial, serial_monitor_stdin};
use kernel::terminal::Writer;
use kernel::time::init_time;
use kernel::uefi::get_config_table;
use kernel::vm::VMO;
use kernel::{BOOT_INFO, elf, gdt, paging};

use bootloader::uefi::table::cfg::{ACPI2_GUID, ConfigTableEntry};

use kernel_sys::syscall::sys_exit;
use kernel_sys::types::{RawValue, VMMapFlags};
use kernel_userspace::channel::Channel;

// #[no_mangle]
entry_point!(main_stage1);

/// The kernels entry point from the bootloader
/// # Safety
/// Must be only called by the bootloader with a valid pointer
pub unsafe fn main_stage1(info: *const BootInfo) -> ! {
    unsafe {
        x86_64::instructions::interrupts::disable();

        // init gdt & idt
        gdt::init_bootgdt();
        interrupts::init_idt();
        init_bsp_boot_ls();

        // Try connecting to COM1
        let mut serial = Serial::new(COM_1);
        if serial.init() {
            // Reset colors, clear screen and move to top left
            serial.write_str("\x1b[0m\x1b[2J\x1b[H");
            serial.write_str("Welcome to Fioxa...\n");
            SERIAL.call_once(|| Spinlock::new(serial));
        }

        let boot_info = info.read();
        // get memory map
        let mmap = MemoryMapIter::new(
            boot_info.mmap_buf,
            boot_info.mmap_entry_size,
            boot_info.mmap_len,
        );

        // set uefi system table
        set_system_table(boot_info.uefi_runtime_table as *const _);

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
            map_lapic(&mut table);
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
    gop::WRITER.call_once(|| Writer::new(boot_info.gop, font).into());
    // Test screen colours
    gop::WRITER.get().unwrap().lock().reset_screen(0xFF_00_00);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_FF_00);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_00_FF);
    gop::WRITER.get().unwrap().lock().reset_screen(0xFF_FF_FF);
    gop::WRITER.get().unwrap().lock().reset_screen(0x00_00_00);

    log::set_logger(&KERNEL_LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    info!("Welcome to Fioxa...");

    unsafe { init_bsp_localstorage() };

    let init_process = ProcessBuilder::new(ProcessMemory::new(), init as *const u64, 0)
        .privilege(ProcessPrivilege::KERNEL)
        .name("INIT PROCESS")
        .build();

    assert!(init_process.pid.into_raw() == 1);

    unsafe { core_start_multitasking() };
}

unsafe fn get_and_map_config_table() -> &'static [ConfigTableEntry] {
    // read boot_info
    let boot_info = unsafe { core::ptr::read(BOOT_INFO) };

    let process = unsafe { CPULocalStorageRW::get_current_task().process() };
    let mut mapper = process.memory.lock();

    let uefi_table_base = (boot_info.uefi_runtime_table as usize) & !0xFFF;
    let uefi_table = unsafe { Arc::new(Spinlock::new(VMO::new_mmap(uefi_table_base, 0x1000))) };

    mapper
        .region
        .map_vmo(uefi_table, VMMapFlags::empty(), Some(uefi_table_base))
        .unwrap();

    info!("UEFI Table addr: {uefi_table_base:#x}");

    info!("Getting UEFI runtime table");

    let st = unsafe { system_table_raw().unwrap().as_ref() };

    info!("Loading UEFI runtime table");

    // map further memory if needed (it might overlap so skip page)
    let ptr = st.configuration_table as usize;
    let len = st.number_of_configuration_table_entries;
    let mut base_addr = ptr & !0xFFF;
    let mut size = ((ptr & 0xFFF) + len * size_of::<ConfigTableEntry>() + 0xFFF) & !0xFFF;

    if base_addr == uefi_table_base {
        base_addr += 0x1000;
        size -= 0x1000;
    }

    if size > 0 {
        let config_tables_mapping =
            unsafe { Arc::new(Spinlock::new(VMO::new_mmap(base_addr, size))) };

        mapper
            .region
            .map_vmo(config_tables_mapping, VMMapFlags::empty(), Some(base_addr))
            .unwrap();
    }

    unsafe { core::slice::from_raw_parts(st.configuration_table.cast(), len) }
}

extern "C" fn init() {
    with_held_interrupts(|| unsafe {
        let config_tables = get_and_map_config_table();
        let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
            .ok_or(AcpiError::NoValidRsdp)
            .and_then(|acpi2_table| {
                acpi::AcpiTables::from_rsdp(FioxaAcpiHandler, acpi2_table.address as usize)
            })
            .unwrap();

        init_time(&acpi_tables);

        let madt = acpi_tables.find_table::<Madt>().unwrap();

        enable_localapic();
        enable_apic(&madt);

        boot_aps(&madt);
    });

    // TODO: Reclaim memory, but first need to drop any references to the memory region
    // unsafe {
    //     let reclaim = frame_alloc_exec(|f| f.reclaim_memory());
    //     println!("RECLAIMED MEMORY: {}Mb", reclaim * 0x1000 / 1024 / 1024);
    // }

    let mut init_handles = Vec::new();

    let mut get_init = || {
        let (l, r) = Channel::new();
        init_handles.push(l);
        ProcessReferences::from_refs(&[**r.handle()])
    };

    spawn_process(run_console).references(get_init()).build();

    spawn_process(check_interrupts)
        .references(get_init())
        .build();

    spawn_process(elf::elf_new_process_loader)
        .references(get_init())
        .build();

    spawn_process(gop::gop_entry).references(get_init()).build();

    spawn_process(userspace_networking_main)
        .references(get_init())
        .build();

    spawn_process(after_boot_pci).references(get_init()).build();

    spawn_process(serial_monitor_stdin)
        .references(get_init())
        .build();

    spawn_process(disk_controller)
        .references(get_init())
        .build();

    spawn_process(fs_controller).references(get_init()).build();

    spawn_process(file_system_partition_loader)
        .references(get_init())
        .build();

    // TODO: Use IO permissions instead of kernel
    load_elf(PS2_DRIVER)
        .unwrap()
        .references(get_init())
        .privilege(ProcessPrivilege::KERNEL)
        .build();

    init_handle_new_proc(init_handles);
    sys_exit();
}

fn after_boot_pci() {
    let config_tables = unsafe { get_and_map_config_table() };

    let acpi_tables = get_config_table(ACPI2_GUID, config_tables)
        .ok_or(AcpiError::NoValidRsdp)
        .unwrap();

    let acpi_tables = unsafe {
        acpi::AcpiTables::from_rsdp(FioxaAcpiHandler, acpi_tables.address as usize).unwrap()
    };

    info!("Enumnerating PCI...");

    enumerate_pci(acpi_tables);

    sys_exit();
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    log!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}
