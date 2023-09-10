#![no_std]
#![no_main]

use core::slice;

use bootloader::{
    fs, gop,
    kernel::load_kernel,
    paging::{clone_pml4, get_uefi_active_mapper},
    BootInfo,
};
use uefi::{
    prelude::{entry, BootServices},
    table::{boot::MemoryType, Boot, SystemTable},
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

fn uefi_entry(mut image_handle: Handle, mut system_table: SystemTable<Boot>) -> ! {
    // Initalize Logger
    let logger = unsafe {
        LOGGER = Some(uefi::logger::Logger::new(system_table.stdout()));
        LOGGER.as_ref().unwrap()
    };

    // Will only fail if allready initialized
    log::set_logger(logger).unwrap();

    // Log everything
    log::set_max_level(log::LevelFilter::Info);

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

    let map = unsafe { clone_pml4(&get_uefi_active_mapper(), boot_services) };
    map.load_into_cr3();

    let stack = unsafe {
        let stack = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, 1024 * 12) // 5 Mb
            .unwrap();
        core::ptr::write_bytes(stack, 0, 1024 * 12);
        stack.add(1024 * 12)
    };

    // Create a memory region to store the boot info in
    let mut boot_info = unsafe { bootloader::get_buffer_as_type::<BootInfo>(boot_services) };

    let entry_point = load_system(&boot_services, &mut image_handle, &mut boot_info);

    let mmap_size = boot_services.memory_map_size();
    boot_info.mmap_entry_size = mmap_size.entry_size;

    // Add a few extra bytes of space, since this allocation will increase the mmap size
    let size = mmap_size.map_size + 0x1000 - 1;
    let mmap_ptr = boot_services
        .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
        .unwrap();

    boot_info.mmap_buf = mmap_ptr;

    let memory_map_buffer = {
        let buffer = unsafe { slice::from_raw_parts_mut(mmap_ptr, size) };
        buffer
    };

    let mut mmap = boot_services.memory_map(memory_map_buffer).unwrap();

    mmap.sort();

    boot_info.mmap_len = mmap.entries().len();

    let (runtime_table, _mmap) = system_table.exit_boot_services();
    // No point printing anything since once we get the GOP buffer the UEFI sdout stops working

    boot_info.uefi_runtime_table = runtime_table.get_current_system_table_addr();

    unsafe {
        core::arch::asm!("mov rsp, {}; push 0; jmp {}", in(reg) stack, in (reg) entry_point, in("rdi") boot_info as *const BootInfo)
    }
    unreachable!()
}

fn load_system(
    boot_services: &BootServices,
    image_handle: &mut Handle,
    boot_info: &mut BootInfo,
) -> u64 {
    info!("Retreiving Root Filesystem...");
    let mut root_fs = unsafe { fs::get_root_fs(boot_services, *image_handle) }.unwrap();

    info!("Retreiving kernel...");

    const KERN_PATH: &str = "fioxa.elf";
    let mut buf = [0; KERN_PATH.len() + 1];
    let kernel_data = fs::read_file(
        boot_services,
        &mut root_fs,
        uefi::CStr16::from_str_with_buf(KERN_PATH, &mut buf).unwrap(),
    )
    .unwrap();

    let entry_point = load_kernel(boot_services, kernel_data, boot_info);

    info!("Initializing GOP...");
    let mut gop = gop::initialize_gop(boot_services);

    let gop_info = gop::get_gop_info(&mut gop);
    boot_info.gop = gop_info;
    entry_point
}
