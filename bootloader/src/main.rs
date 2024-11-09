#![no_std]
#![no_main]

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
    uefi::helpers::init(&mut system_table).unwrap();

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
            .allocate_pool(MemoryType::LOADER_DATA, 0x1000 * 25) // 100 KB
            .unwrap();
        core::ptr::write_bytes(stack.as_ptr(), 0, 0x1000 * 25);
        stack.add(0x1000 * 25)
    };

    // Create a memory region to store the boot info in
    let mut boot_info = unsafe { bootloader::get_buffer_as_type::<BootInfo>(boot_services) };

    let entry_point = load_system(&boot_services, &mut image_handle, &mut boot_info);

    let (runtime_table, mut mmap) =
        unsafe { system_table.exit_boot_services(MemoryType::LOADER_DATA) };
    // No point printing anything since once we get the GOP buffer the UEFI sdout stops working

    mmap.sort();

    let mmap_raw = mmap.as_raw();

    boot_info.mmap_buf = mmap_raw.0.as_ptr();
    boot_info.mmap_len = mmap_raw.1.entry_count();
    boot_info.mmap_entry_size = mmap_raw.1.desc_size;

    boot_info.uefi_runtime_table = runtime_table.get_current_system_table_addr();

    unsafe {
        core::arch::asm!("mov rsp, {}; push 0; jmp {}", in(reg) stack.as_ptr(), in (reg) entry_point, in("rdi") boot_info as *const BootInfo)
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
