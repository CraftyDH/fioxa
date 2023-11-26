#![no_std]
#![no_main]

use bootloader::{
    fs, gop,
    kernel::load_kernel,
    paging::{clone_pml4, get_uefi_active_mapper},
    BootInfo, MemoryClass, MemoryMapEntry, MemoryMapEntrySlice, KERNEL_MEMORY, KERNEL_RECLAIM,
};
use uefi::{
    prelude::{entry, BootServices},
    table::{
        boot::{AllocateType, MemoryType},
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
            .allocate_pages(AllocateType::AnyPages, KERNEL_RECLAIM, 25) // 100 KB
            .unwrap() as *mut u8;
        core::ptr::write_bytes(stack, 0, 0x1000 * 25);
        stack.add(0x1000 * 25)
    };

    // Create a memory region to store the boot info in
    let mut boot_info = unsafe { bootloader::get_buffer_as_type::<BootInfo>(boot_services) };

    let entry_point = load_system(&boot_services, &mut image_handle, &mut boot_info);

    let mmap_size = boot_services.memory_map_size();

    // Add a few extra zones to ensure that we will be able to describe everything
    let max_entries = (mmap_size.map_size + 10) / mmap_size.entry_size;

    let mmap_ptr = boot_services
        .allocate_pool(
            KERNEL_RECLAIM,
            max_entries * core::mem::size_of::<MemoryMapEntry>(),
        )
        .unwrap();

    let mut memory_map_buffer =
        unsafe { MemoryMapEntrySlice::new(mmap_ptr as *mut MemoryMapEntry, max_entries) };

    let (runtime_table, mut mmap) = system_table.exit_boot_services();
    // No point printing anything since once we get the GOP buffer the UEFI sdout stops working

    boot_info.uefi_runtime_table = runtime_table.get_current_system_table_addr();

    mmap.sort();

    let mut entries = mmap.entries().peekable();
    while let Some(e) = entries.next() {
        let mut page_count = e.page_count;

        let class = get_memtype_class(e.ty);

        // Colapse entries which are usability
        while let Some(next) = entries.peek() {
            if e.phys_start + page_count * 0x1000 == next.phys_start
                && get_memtype_class(next.ty) == class
            {
                page_count += entries.next().unwrap().page_count;
            } else {
                break;
            }
        }

        memory_map_buffer.push(MemoryMapEntry {
            class,
            phys_start: e.phys_start,
            page_count,
        });
    }

    boot_info.mmap = memory_map_buffer;

    unsafe {
        core::arch::asm!("mov rsp, {}; push 0; jmp {}", in(reg) stack, in (reg) entry_point, in("rdi") boot_info as *const BootInfo)
    }
    unreachable!()
}

fn get_memtype_class(ty: MemoryType) -> MemoryClass {
    if ty == MemoryType::CONVENTIONAL
        || ty == MemoryType::BOOT_SERVICES_CODE
        || ty == MemoryType::BOOT_SERVICES_DATA
        || ty == MemoryType::LOADER_CODE
        || ty == MemoryType::LOADER_DATA
    {
        MemoryClass::Free
    } else if ty == KERNEL_RECLAIM {
        MemoryClass::KernelReclaim
    } else if ty == KERNEL_MEMORY {
        MemoryClass::KernelMemory
    } else {
        MemoryClass::Unusable
    }
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
