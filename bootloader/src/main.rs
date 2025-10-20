#![no_std]
#![no_main]

use alloc::boxed::Box;
use bootloader::{
    BootInfo, fs, gop,
    kernel::load_kernel,
    paging::{clone_pml4, get_uefi_active_mapper},
};
use uefi::{
    Status,
    boot::{MemoryType, allocate_pool, exit_boot_services},
    mem::memory_map::{MemoryMap, MemoryMapMut},
    prelude::entry,
    table::system_table_raw,
};

#[macro_use]
extern crate log;

extern crate alloc;

#[entry]
fn uefi_entry() -> Status {
    uefi::helpers::init().unwrap();

    // Log everything
    log::set_max_level(log::LevelFilter::Info);

    // If run on debug mode show debug messages
    #[cfg(debug_assertions)]
    log::set_max_level(log::LevelFilter::Debug);

    info!("Starting Fioxa bootloader...");

    let map = unsafe { clone_pml4(&get_uefi_active_mapper()) };
    map.load_into_cr3();

    let stack = unsafe {
        let stack = allocate_pool(MemoryType::LOADER_DATA, 0x1000 * 25) // 100 KB
            .unwrap();
        core::ptr::write_bytes(stack.as_ptr(), 0, 0x1000 * 25);
        stack.add(0x1000 * 25)
    };

    // Create a memory region to store the boot info in
    let mut boot_info = unsafe { Box::<BootInfo>::new_uninit().assume_init() };

    let entry_point = load_system(&mut boot_info);

    let mut mmap = unsafe { exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    // No point printing anything since once we get the GOP buffer the UEFI sdout stops working

    mmap.sort();

    boot_info.mmap_buf = mmap.buffer().as_ptr();
    boot_info.mmap_len = mmap.len();
    boot_info.mmap_entry_size = mmap.meta().desc_size;

    boot_info.uefi_runtime_table = system_table_raw().unwrap().as_ptr() as u64;

    unsafe {
        core::arch::asm!(
            "mov rsp, {}; push 0; jmp {}",
            in(reg) stack.as_ptr(),
            in(reg) entry_point,
            in("rdi") boot_info.as_ref() as *const _,
            options(noreturn)
        )
    }
}

fn load_system(boot_info: &mut BootInfo) -> u64 {
    info!("Retrieving Root Filesystem...");
    let mut root_fs = unsafe { fs::get_root_fs() }.unwrap();

    info!("Retrieving kernel...");

    const KERN_PATH: &str = "fioxa.elf";
    let mut buf = [0; KERN_PATH.len() + 1];
    let kernel_data = fs::read_file(
        &mut root_fs,
        uefi::CStr16::from_str_with_buf(KERN_PATH, &mut buf).unwrap(),
    )
    .unwrap();

    let entry_point = load_kernel(&kernel_data, boot_info);

    info!("Initializing GOP...");
    let mut gop = gop::initialize_gop();

    let gop_info = gop::get_gop_info(&mut gop);
    boot_info.gop = gop_info;
    entry_point
}
