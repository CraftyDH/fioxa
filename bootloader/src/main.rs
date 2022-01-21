#![no_std]
#![no_main]
#![feature(abi_efiapi)]
#![feature(asm)]
#![feature(slice_pattern)]

mod fs;
mod gop;
mod kernel;
mod psf1;

use core::slice::{self};

use alloc::vec::Vec;
use types::{BootInfo, RSDP2};
use uefi::{
    prelude::entry,
    table::{
        boot::{MemoryDescriptor, MemoryType},
        Boot, SystemTable,
    },
    Handle, ResultExt, Status,
};

use crate::kernel::load_kernel;

#[macro_use]
extern crate log;

extern crate alloc;

#[entry]
fn _start(image_handle: Handle, system_table: SystemTable<Boot>) -> Status {
    uefi_entry(image_handle, system_table)
}

fn uefi_entry(image_handle: Handle, mut system_table: SystemTable<Boot>) -> ! {
    // Initalize UEFI boot services
    uefi_services::init(&mut system_table).unwrap_success();

    // If run on debug mode show debug messages
    #[cfg(debug_assertions)]
    log::set_max_level(log::LevelFilter::Debug);

    // Clear the screen
    system_table
        .stdout()
        .reset(false)
        .expect_success("Failed to reset output buffer");

    info!("Starting Fioxa bootloader...");

    let boot_services = system_table.boot_services();

    info!("Initializing GOP...");
    let gop = gop::initialize_gop(boot_services);

    info!("Retreiving Root Filesystem...");
    let mut root_fs = fs::get_root_fs(boot_services, image_handle);

    info!("Retreiving kernel...");

    let kernel_data = fs::read_file(boot_services, &mut root_fs, "fioxa.elf").unwrap();

    info!("Loading PSF1 Font...");
    let font = psf1::load_psf1_font(boot_services, &mut root_fs, "font.psf");

    let entry_point = load_kernel(boot_services, kernel_data);

    let gop_info = gop::get_gop_info(gop);

    let mut boot_info = {
        let size = core::mem::size_of::<BootInfo>();
        let ptr = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { &mut *(ptr as *mut BootInfo) }
    };

    boot_info.gop = gop_info;
    boot_info.font = font;
    // Exit boot services

    let mut memory_map_buffer = {
        let size = boot_services.memory_map_size() + 8 * core::mem::size_of::<MemoryDescriptor>();
        let ptr = boot_services
            .allocate_pool(MemoryType::BOOT_SERVICES_DATA, size)
            .unwrap()
            .unwrap();
        let buffer = unsafe { slice::from_raw_parts_mut(ptr, size) };
        buffer
    };

    let (_key, mmap) = boot_services
        .memory_map(memory_map_buffer)
        .unwrap()
        .unwrap();

    let x: Vec<MemoryDescriptor> = mmap.copied().collect();
    let system_table_cop = unsafe { system_table.unsafe_clone() };
    let config_table = system_table_cop.config_table();

    // Find RSDP
    for entry in config_table {
        // We want last correct entry so keep interating
        if entry.guid == uefi::table::cfg::ACPI2_GUID {
            boot_info.rsdp = unsafe { &*(entry.address as *const RSDP2) };
        }
    }

    let (_runtime_table, _) =
        match system_table.exit_boot_services(image_handle, &mut memory_map_buffer) {
            Ok(table) => table.unwrap(),
            Err(e) => {
                error!("Error: {:?}", e);
                loop {}
            }
        };

    boot_info.mmap = x.as_slice();

    // let kernel_entry: fn(BootInfo) -> u64 =
    //     unsafe { core::mem::transmute(entry_point as *const ()) };

    unsafe { asm!("push 0; jmp {}", in (reg) entry_point, in("rdi") boot_info as *const BootInfo) }

    unreachable!()
}
