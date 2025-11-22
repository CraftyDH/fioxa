use core::sync::atomic::AtomicU32;

use kernel_sys::types::VMMapFlags;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{
        MemoryLoc,
        page::{Page, Size4KB},
        page_allocator::global_allocator,
        page_table::{MapMemoryError, Mapper, PageTable, TableLevel4},
    },
    scheduling::{taskmanager::enter_sched, with_held_interrupts},
    time::{HPET, check_sleep},
};

// Local APIC
pub const PHYS_LAPIC_ADDR: u64 = 0xfee00000;
pub const LAPIC_ADDR: u64 = MemoryLoc::PhysMapOffset as u64 + PHYS_LAPIC_ADDR;

pub fn map_lapic(mapper: &mut PageTable<TableLevel4>) {
    match mapper.map(
        global_allocator(),
        Page::<Size4KB>::new(LAPIC_ADDR),
        Page::new(PHYS_LAPIC_ADDR),
        VMMapFlags::WRITEABLE,
    ) {
        Ok(f) => f.flush(),
        Err(MapMemoryError::MemAlreadyMapped {
            from: _,
            to,
            current,
        }) if to == current => info!("LAPIC was already mapped"),
        Err(e) => panic!("cannot ident map because {e:?}"),
    }
}

unsafe fn write_lapic(offset: u64, val: u32) {
    let addr = (LAPIC_ADDR + offset) as *mut u32;
    unsafe { addr.write_volatile(val) };
}

unsafe fn read_lapic(offset: u64) -> u32 {
    let addr = (LAPIC_ADDR + offset) as *mut u32;
    unsafe { addr.read_volatile() }
}

pub static LAPIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(0);

pub unsafe fn enable_localapic() {
    with_held_interrupts(|| unsafe {
        // Enable + Spurious vector
        write_lapic(0xF0, 1 << 8 | 0xFF);

        // Ack any pending interrupt
        *((LAPIC_ADDR + 0xb0) as *mut u32) = 0;

        // set timer divisor of 16
        write_lapic(0x3E0, 0x3);

        // measure ticks per ms.
        // we just want something close to a ms for task switching, we will use HPET for all time tracking
        write_lapic(0x380, 0xFFFFFFFF);

        HPET.get().unwrap().spin_ms(1);

        let ticks_per_ms = 0xFFFFFFFF - read_lapic(0x390);
        trace!("LAPIC Ticks per ms: {ticks_per_ms}");
        LAPIC_TICKS_PER_MS.store(ticks_per_ms, core::sync::atomic::Ordering::SeqCst);

        // set timer vector + periodic mode
        write_lapic(0x320, 60 | 0x20000);

        // set timer divisor of 16
        write_lapic(0x3E0, 0x3);

        // set timer count
        write_lapic(0x380, ticks_per_ms);
    });
}

pub unsafe fn disable_localapic() {
    unsafe {
        // mask timer
        write_lapic(0x320, 1 << 16);

        // disable lapic
        write_lapic(0xF0, 1 << 18);
    }
}

pub extern "x86-interrupt" fn tick_handler(_: InterruptStackFrame) {
    unsafe {
        if !HPET.is_completed() {
            // Normally this shouldn't be possible as we init time before lapic,
            // But if we kexec there might exist a pending interrupt from before the reset.
            warn!("Spurious LAPIC timer interrupt");
            return;
        }

        // Ack interrupt
        *((LAPIC_ADDR + 0xb0) as *mut u32) = 0;

        check_sleep();

        // if we are not in sched yield to it
        if CPULocalStorageRW::get_context() > 0 {
            let mut sched = CPULocalStorageRW::get_current_task().sched().lock();
            enter_sched(&mut sched);
        }
    }
}
