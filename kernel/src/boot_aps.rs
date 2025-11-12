use core::{
    arch::x86_64::{__cpuid, _mm_pause},
    ptr::{read_volatile, write_volatile},
    sync::atomic::AtomicU32,
};

use alloc::sync::Arc;
use kernel_sys::types::VMMapFlags;

use crate::{
    assembly::AP_TRAMPOLINE,
    cpu_localstorage::{CPULocalStorageRW, new_cpu},
    gdt::CPULocalGDT,
    interrupts::IDT,
    ioapic::Madt,
    lapic::{LAPIC_ADDR, enable_localapic},
    mutex::Spinlock,
    paging::{
        KERNEL_LVL4, MemoryLoc,
        page::{Page, Size4KB},
        page_allocator::{frame_alloc_exec, global_allocator},
        page_table::Mapper,
    },
    scheduling::taskmanager::core_start_multitasking,
    time::spin_sleep_ms,
    vm::VMO,
};

/// It is assumed that 0x8000 is identity mapped before this point
pub unsafe fn boot_aps(madt: &Madt) {
    if !frame_alloc_exec(|a| a.captured_0x8000()) {
        warn!(
            "WARNING: SINGLE CORE BOOT -- The physical memory region `0x8000` was not available during initialization."
        );
        return;
    }

    // Get current core id
    let bsp_addr = (unsafe { __cpuid(1) }.ebx >> 24) as u8;

    let task = unsafe { CPULocalStorageRW::get_current_task() };

    let cr3_addr = {
        let mut mem = task.process().memory.lock();
        let mut kernel_mem = KERNEL_LVL4.lock();

        // we need to set 0x8000 for the trampoline
        unsafe {
            mem.region
                .map_vmo(
                    Arc::new(Spinlock::new(VMO::new_mmap(0x8000, 0x1000))),
                    VMMapFlags::WRITEABLE,
                    Some(0x8000),
                )
                .unwrap();
        };
        kernel_mem
            .identity_map(
                global_allocator(),
                Page::<Size4KB>::new(0x8000),
                VMMapFlags::WRITEABLE,
            )
            .unwrap()
            .ignore();

        let Ok(addr) = kernel_mem.get_physical_address().try_into() else {
            error!("KERNEL MAP SHOULD BE 32bits for AP BOOT");
            return;
        };
        addr
    };

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
        *((end + 8) as *mut u32) = cr3_addr;
        *((end + 16) as *mut usize) = ap_startup_f as usize;
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
        info!("Booting Core: {id}");

        let local_storage = unsafe { new_cpu(id) };

        let id = id as usize;

        unsafe { core_local_storage.add(id).write(local_storage) };

        unsafe {
            // Select AP
            write_volatile(apic_ipi_310, (id << 24) as u32);
            // Trigger INIT IPI
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
        debug!("{c}/{n_cores} cores booted...");
        if c as usize == n_cores {
            break;
        };
        _mm_pause();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ap_startup_f(core_id: u32) {
    let vaddr_base = MemoryLoc::PerCpuMem as u64 + 0x100_0000 * core_id as u64;

    unsafe {
        let gdt = &mut *((vaddr_base + 0x1000) as *mut CPULocalGDT);

        // Load GDT
        gdt.load();

        // Load IDT
        IDT.lock().load_unsafe();

        // Enable lapic
        enable_localapic();
    }

    info!("Core: {core_id} booted");

    // loop {}
    unsafe { core_start_multitasking() }
}
