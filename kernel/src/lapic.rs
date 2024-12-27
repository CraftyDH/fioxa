use core::sync::atomic::AtomicU32;

use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{
        page::{Page, Size4KB},
        page_allocator::global_allocator,
        page_table::{MapMemoryError, Mapper, PageTable, TableLevel4},
        MemoryMappingFlags,
    },
    scheduling::{taskmanager::enter_sched, with_held_interrupts},
    time::{check_sleep, HPET},
};

// Local APIC
/// Do not use before this has been initialized in enable_apic
pub const LAPIC_ADDR: u64 = 0xfee00000;

pub fn map_lapic(mapper: &mut PageTable<TableLevel4>) {
    match mapper.identity_map(
        global_allocator(),
        Page::<Size4KB>::new(0xfee00000),
        MemoryMappingFlags::WRITEABLE,
    ) {
        Ok(f) => f.flush(),
        Err(MapMemoryError::MemAlreadyMapped {
            from: _,
            to,
            current,
        }) if to == current => (),
        Err(e) => panic!("cannot ident map because {e:?}"),
    }
}

unsafe fn write_lapic(offset: u64, val: u32) {
    let addr = (LAPIC_ADDR + offset) as *mut u32;
    addr.write_volatile(val);
}

unsafe fn read_lapic(offset: u64) -> u32 {
    let addr = (LAPIC_ADDR + offset) as *mut u32;
    addr.read_volatile()
}

pub static LAPIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(0);

pub unsafe fn enable_localapic() {
    with_held_interrupts(|| {
        // Enable + Spurious vector
        write_lapic(0xF0, 1 << 8 | 0xFF);

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

pub extern "x86-interrupt" fn tick_handler(_: InterruptStackFrame) {
    unsafe {
        // Ack interrupt
        *(0xfee000b0 as *mut u32) = 0;

        check_sleep();

        // if we are not in sched yield to it
        if CPULocalStorageRW::get_context() > 0 {
            let mut sched = CPULocalStorageRW::get_current_task().sched().lock();
            enter_sched(&mut sched);
        }
    }
}
