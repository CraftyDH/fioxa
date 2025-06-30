use core::sync::atomic::AtomicU32;

use kernel_sys::types::VMMapFlags;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    cpu_localstorage::CPULocalStorage,
    paging::{
        MemoryLoc,
        page::{Page, Size4KB},
        page_allocator::global_allocator,
        page_table::{MapMemoryError, Mapper, PageTable, TableLevel4},
    },
    scheduling::{process::VMExitStateID, with_held_interrupts},
    time::HPET,
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

#[unsafe(naked)]
pub extern "x86-interrupt" fn tick_handler(_: InterruptStackFrame) {
    // Only context switch if in context 2 (AKA userspace)
    core::arch::naked_asm!(
        "cmp byte ptr gs:{ctx}, 2",
        "je 2f",

        // ack interrupt
        "push rax",
        "mov rax, {clr_int}",
        "mov dword ptr [rax], 0",
        "pop rax",
        "iretq",

        "2:",
        // registers
        "push rbp",
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // ack interrupt
        "mov rax, {clr_int}",
        "mov dword ptr [rax], 0",

        "mov rdx, rsp",
        "mov rsp, gs:{vm_sp}",
        "mov eax, {exit_type}",
        "ret",
        clr_int = const LAPIC_ADDR + 0xb0,
        ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
        vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
        exit_type = const VMExitStateID::Complete as u32,
    );
}
