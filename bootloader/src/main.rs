#![no_std]
#![no_main]
#![feature(abi_efiapi)]

use core::slice;

use bootloader::{fs, gop, kernel::load_kernel, psf1, BootInfo};
use uefi::{
    prelude::entry,
    table::{
        boot::{MemoryDescriptor, MemoryType},
        Boot, SystemTable,
    },
    Handle, Status,
};

/// Global logger object
static mut LOGGER: Option<uefi::logger::Logger> = None;

#[macro_use]
extern crate log;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    error!("Panic: {}", info);
    loop {}
}

#[entry]
fn _start(image_handle: Handle, system_table: SystemTable<Boot>) -> Status {
    uefi_entry(image_handle, system_table)
}

fn uefi_entry(image_handle: Handle, mut system_table: SystemTable<Boot>) -> ! {
    // Initalize Logger
    let logger = unsafe {
        LOGGER = Some(uefi::logger::Logger::new(system_table.stdout()));
        LOGGER.as_ref().unwrap()
    };

    // Will only fail if allready initialized
    log::set_logger(logger).unwrap();

    // Log everything
    log::set_max_level(log::LevelFilter::Info);

    // Initalize UEFI boot services
    // uefi_services::init(&mut system_table).unwrap_success();

    // If run on debug mode show debug messages
    #[cfg(debug_assertions)]
    log::set_max_level(log::LevelFilter::Debug);

    // Clear the screen
    system_table
        .stdout()
        .reset(false)
        .expect("Failed to reset output buffer");

    info!("Starting Fioxa bootloader...");

    let boot_services = system_table.boot_services();

    info!("Initializing GOP...");
    let gop = &mut *gop::initialize_gop(boot_services);

    info!("Retreiving Root Filesystem...");
    let mut root_fs = unsafe { fs::get_root_fs(boot_services, image_handle) };

    info!("Retreiving kernel...");

    const KERN_PATH: &str = "fioxa.elf";
    let mut buf = [0; KERN_PATH.len() + 1];
    let kernel_data = fs::read_file(
        boot_services,
        &mut root_fs,
        uefi::CStr16::from_str_with_buf(KERN_PATH, &mut buf).unwrap(),
    )
    .unwrap();

    info!("Loading PSF1 Font...");
    const FONT_PATH: &str = "font.psf";
    let mut buf = [0; FONT_PATH.len() + 1];
    let font = psf1::load_psf1_font(
        boot_services,
        &mut root_fs,
        uefi::CStr16::from_str_with_buf(FONT_PATH, &mut buf).unwrap(),
    );

    let entry_point = load_kernel(boot_services, kernel_data);

    let gop_info = gop::get_gop_info(gop);

    // Create a memory region to store the boot info in
    let mut boot_info = {
        let size = core::mem::size_of::<BootInfo>();
        let ptr = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
            .unwrap();
        unsafe { &mut *(ptr as *mut BootInfo) }
    };

    boot_info.gop = gop_info;
    boot_info.font = font;

    let stack = unsafe {
        let stack = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, 1024 * 1024 * 10) // 10 Mb
            .unwrap();
        core::ptr::write_bytes(stack, 0, 1024 * 1024 * 10);
        stack.add(1024 * 1024 * 10)
    };

    let memory_map_buffer = {
        let size = boot_services.memory_map_size().map_size;
        let ptr = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
            .unwrap();
        let buffer = unsafe { slice::from_raw_parts_mut(ptr, size) };
        buffer
    };
    let (_key, mmap) = boot_services.memory_map(memory_map_buffer).unwrap();

    // Collect mmap into a slice
    let mmap_buf = {
        let size = mmap.len() * core::mem::size_of::<MemoryDescriptor>();
        let ptr = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
            .unwrap() as *mut MemoryDescriptor;
        let buffer = unsafe { slice::from_raw_parts_mut(ptr, mmap.len()) };
        buffer
    };

    for (i, m) in mmap.enumerate() {
        mmap_buf[i] = *m;
    }

    boot_info.mmap = mmap_buf.as_mut_ptr();
    boot_info.mmap_size = mmap_buf.len();

    let config_table = system_table.config_table();

    // Ensure a successful init
    boot_info.rsdp_address = None;
    // Find RSDP
    for entry in config_table {
        // We want last correct entry so keep interating
        if entry.guid == uefi::table::cfg::ACPI2_GUID {
            boot_info.rsdp_address = Some(entry.address as usize);
        }
    }

    let (_runtime_table, _) = match system_table.exit_boot_services(image_handle, memory_map_buffer)
    {
        Ok(table) => table,
        Err(e) => {
            error!("Error: {:?}", e);
            loop {}
        }
    };

    // let kernel_entry: fn(*const BootInfo) -> u64 =
    //     unsafe { core::mem::transmute(entry_point as *const ()) };

    // kernel_entry(boot_info as *const BootInfo);

    unsafe {
        core::arch::asm!("mov rsp, {}; push 0; jmp {}", in(reg) stack, in (reg) entry_point, in("rdi") boot_info as *const BootInfo)
    }

    unreachable!()
}
