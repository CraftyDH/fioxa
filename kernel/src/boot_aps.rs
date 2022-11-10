use core::{
    arch::{
        global_asm,
        x86_64::{__cpuid, _mm_pause},
    },
    ptr::{read_volatile, write_volatile},
    sync::atomic::AtomicU32,
};

use crate::{
    ioapic::Madt,
    assembly::{ap_trampoline, ap_trampoline_end},
    cpu_localstorage::{init_bsp_task, new_cpu},
    gdt,
    hpet::spin_sleep_ms,
    interrupts::IDT,
    lapic::{enable_localapic, LAPIC_ADDR},
    paging::{
        get_uefi_active_mapper,
        identity_map::FULL_IDENTITY_MAP,
        page_allocator::frame_alloc_exec,
        page_table_manager::{ident_map_curr_process, PageTableManager},
    },
    pit::start_switching_tasks,
    scheduling::taskmanager::{core_start_multitasking, TASKMANAGER},
};

#[no_mangle]
pub static mut bspdone: u32 = 0;
#[no_mangle]
pub static aprunning: AtomicU32 = AtomicU32::new(1);
#[no_mangle]
pub static mut ap_startup: u64 = 0;

pub fn boot_aps(madt: &Madt) {
    // Get current core id
    let bsp_addr = (unsafe { __cpuid(1) }.ebx >> 24) as u8;
    frame_alloc_exec(|m| m.lock().lock_reserved_16bit_page(0x8000)).unwrap();
    ident_map_curr_process(0x8000, true);

    unsafe { init_bsp_task() };

    unsafe {
        ap_startup = ap_startup_f as u64;
    }
    println!("AP: {}", ap_startup_f as u64);
    let stack_ptr;
    unsafe {
        core::ptr::copy(
            ap_trampoline as u64 as *mut u8,
            0x8000 as *mut u8,
            ap_trampoline_end as usize,
        );
        stack_ptr = (0x8000 + ap_trampoline_end) as *mut u64;
    }

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

        unsafe { stack_ptr.add(id).write(local_storage) };

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
        bspdone = 1;
    }

    let n_cores = lapic_ids.len();
    assert!(
        &(n_cores as u8 - 1) == lapic_ids.iter().max().unwrap(),
        "CPU core id's are not linear"
    );

    loop {
        let c = aprunning.load(core::sync::atomic::Ordering::SeqCst);
        println!("{c}/{n_cores} cores booted...");
        if c as usize == n_cores {
            break;
        };
        unsafe { _mm_pause() }
    }

    unsafe {
        let mapper = PageTableManager::new(FULL_IDENTITY_MAP.lock().get_lvl4_addr());
        TASKMANAGER.lock().init(mapper, n_cores.try_into().unwrap());
    }
    start_switching_tasks();

    unsafe {
        core::ptr::copy(nop_task as u64 as *mut u8, 0x1000 as *mut u8, 10);
    }
}

extern "C" {
    fn nop_task();
}

global_asm!(
    ".global nop_task
    nop_task:
        cli
        jmp nop_task
    "
);

#[no_mangle]
pub extern "C" fn ap_startup_f(core_id: u32) {
    // Load IDT
    unsafe { IDT.lock().load_unsafe() };

    // Load GDT
    gdt::init(core_id as usize);

    let mut mapper = unsafe { get_uefi_active_mapper() };

    enable_localapic(&mut mapper);
    println!("Core: {core_id} booted");

    // loop {}
    unsafe { core_start_multitasking() }
}
