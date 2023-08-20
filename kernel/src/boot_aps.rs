use core::{
    arch::x86_64::{__cpuid, _mm_pause},
    ptr::{read_volatile, write_volatile},
    sync::atomic::AtomicU32,
};

use crate::{
    assembly::AP_TRAMPOLINE,
    cpu_localstorage::new_cpu,
    gdt::CPULocalGDT,
    interrupts::IDT,
    ioapic::Madt,
    lapic::{enable_localapic, LAPIC_ADDR},
    paging::{
        get_uefi_active_mapper,
        page_allocator::frame_alloc_exec,
        page_table_manager::{Mapper, Page, PageTable, Size4KB},
        MemoryLoc, KERNEL_MAP,
    },
    scheduling::taskmanager::{self, core_start_multitasking},
    time::spin_sleep_ms,
};

pub fn boot_aps(madt: &Madt) {
    // Get current core id
    let bsp_addr = (unsafe { __cpuid(1) }.ebx >> 24) as u8;
    frame_alloc_exec(|m| m.lock_reserved_16bit_page(0x8000)).unwrap();

    KERNEL_MAP
        .lock()
        .identity_map_memory(Page::<Size4KB>::new(0x8000))
        .unwrap()
        .flush();

    let bspdone;
    let aprunning;
    let core_local_storage;
    unsafe {
        core::ptr::copy(
            AP_TRAMPOLINE.as_ptr(),
            0x8000 as *mut u8,
            AP_TRAMPOLINE.len(),
        );
        let end = 0x8000 + AP_TRAMPOLINE.len();
        bspdone = (end) as *mut u32;
        aprunning = &mut *((end + 4) as *mut AtomicU32);
        *((end + 8) as *mut u32) = KERNEL_MAP
            .lock()
            .into_page()
            .get_address()
            .try_into()
            .expect("KERNEL MAP SHOULD BE 32bits for AP BOOT");
        *((end + 16) as *mut u64) = ap_startup_f as u64;
        core_local_storage = (end + 24) as *mut u64;
    }

    // We as BSP are running
    aprunning.store(1, core::sync::atomic::Ordering::Relaxed);

    let apic_ipi_300 = (LAPIC_ADDR + 0x300) as *mut u32;
    let apic_ipi_310 = (LAPIC_ADDR + 0x310) as *mut u32;

    let lapic_ids = madt.get_lapid_ids();

    for core in lapic_ids.iter() {
        if *core == bsp_addr {
            continue;
        };
        let id = *core;
        println!("Booting Core: {id}");

        let local_storage = unsafe { new_cpu(id) };

        let id = id as usize;

        unsafe { core_local_storage.add(id).write(local_storage) };

        unsafe {
            // Select AP
            write_volatile(apic_ipi_310, (id << 24) as u32);
            // Tigger INIT IPI
            write_volatile(apic_ipi_300, 0x4500);
            // Wait for delivery
            while read_volatile(apic_ipi_300) & (1 << 12) > 0 {
                _mm_pause()
            }
            //* Sleep 10ms
            spin_sleep_ms(10);

            //* We are supposed to send the startup ipi twice
            for _ in 0..1 {
                // Send START IPI
                // Select AP
                write_volatile(apic_ipi_310, (id << 24) as u32);
                // Trigger STARTUP IPI for 0800:0000
                write_volatile(apic_ipi_300, 0x4600 | 8);
                // Wait 200 usec
                spin_sleep_ms(1);
                // Wait for delivery
                while read_volatile(apic_ipi_300) & (1 << 12) > 0 {
                    _mm_pause()
                }
            }
            spin_sleep_ms(10);
        }
    }

    unsafe {
        *bspdone = 1;
    }

    let n_cores = lapic_ids.len();

    loop {
        let c = aprunning.load(core::sync::atomic::Ordering::SeqCst);
        println!("{c}/{n_cores} cores booted...");
        if c as usize == n_cores {
            break;
        };
        unsafe { _mm_pause() }
    }

    unsafe {
        let mapper = PageTable::from_page(KERNEL_MAP.lock().into_page());

        taskmanager::init(mapper, n_cores.try_into().unwrap());
    }

    KERNEL_MAP
        .lock()
        .unmap_memory(Page::<Size4KB>::new(0x8000))
        .unwrap()
        .flush();
}

#[no_mangle]
pub extern "C" fn ap_startup_f(core_id: u32) {
    let vaddr_base = MemoryLoc::PerCpuMem as u64 + 0x100_0000 * core_id as u64;

    unsafe {
        let gdt = &mut *((vaddr_base + 0x1000) as *mut CPULocalGDT);

        // Load GDT
        gdt.load();

        // Load IDT
        IDT.lock().load_unsafe();

        // Enable lapic
        let mut mapper = get_uefi_active_mapper();
        enable_localapic(&mut mapper);
    }

    println!("Core: {core_id} booted");

    // loop {}
    unsafe { core_start_multitasking() }
}
