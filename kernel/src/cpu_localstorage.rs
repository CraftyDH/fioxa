use core::mem::size_of;

use crate::{
    gdt::CPULocalGDT,
    paging::{
        get_uefi_active_mapper,
        page_allocator::{frame_alloc_exec, request_page},
        page_table_manager::{page_4kb, Mapper},
        virt_addr_for_phys, MemoryLoc,
    },
    scheduling::process::{PID, TID},
};

#[repr(C, packed)]
pub struct CPULocalStorage {
    core_id: u8,
    stack_top: u64,
    task_mgr_current_pid: PID,
    task_mgr_current_tid: TID,
    task_mgr_ticks_left: u32,
    // If not set the task should stay scheduled
    task_mgr_schedule: u32,
    // at 0x1000 (1 page down is GDT)
}

pub unsafe fn init_core(core_id: u8) -> u64 {
    let vaddr_base = MemoryLoc::PerCpuMem as u64 + 0x100_0000 * core_id as u64;

    let cpu_lsize = size_of::<CPULocalStorage>() as u64;
    let gdt_size = size_of::<CPULocalGDT>() as u64;
    assert!(cpu_lsize <= 0x1000);
    // Amount of cpu storage we have
    assert!(gdt_size + 0x1000 <= 0x10_0000);

    let mut map = get_uefi_active_mapper();

    for page in (vaddr_base..vaddr_base + gdt_size + 0x1FFF).step_by(0x1000) {
        let phys = request_page().unwrap();
        map.map_memory(page_4kb(page), page_4kb(phys))
            .unwrap()
            .flush();
    }

    let ls = unsafe { &mut *(vaddr_base as *mut CPULocalStorage) };
    ls.core_id = core_id;
    ls.task_mgr_current_pid = 0.into();
    ls.task_mgr_current_tid = (core_id as u64).into();
    ls.task_mgr_ticks_left = 0;
    ls.task_mgr_schedule = 1;

    crate::gdt::create_gdt_for_core(unsafe { &mut *((vaddr_base + 0x1000) as *mut CPULocalGDT) });

    vaddr_base
}

pub unsafe fn init_bsp_task() {
    let gs_base = init_core(0);

    // Load new core GDT
    // TODO: Remove old GDT
    let gdt = unsafe { &mut *((gs_base + 0x1000) as *mut CPULocalGDT) };

    unsafe { gdt.load() };

    let gs_upper = (gs_base >> 32) as u32;
    let gs_lower = gs_base as u32;

    unsafe {
        core::arch::asm!(
            "
            mov gs, {0:e}
            mov ecx, 0xC0000101
            wrmsr
            mov ecx, 0xC0000102
            wrmsr
            ", in(reg) 0, in("edx") gs_upper, in("eax") gs_lower, lateout("edx") _,  lateout("ecx") _
        )
    }
}

pub const CPU_STACK_SIZE: u64 = 0x1000 * 10;

pub unsafe fn new_cpu(core_id: u8) -> u64 {
    let vaddr = init_core(core_id);
    let ls = unsafe { &mut *(vaddr as *mut CPULocalStorage) };

    let stack_base = frame_alloc_exec(|c| c.request_cont_pages(10)).unwrap();

    ls.stack_top = virt_addr_for_phys(stack_base) + CPU_STACK_SIZE;
    vaddr
}

pub fn get_current_cpu_id() -> u8 {
    let cid: u16;
    unsafe { core::arch::asm!("mov {:e}, gs:0", lateout(reg) cid) };
    cid as u8
}

pub fn get_task_mgr_current_pid() -> PID {
    let pid: u64;
    unsafe { core::arch::asm!("mov {}, gs:9", lateout(reg) pid) };
    pid.into()
}

pub fn set_task_mgr_current_pid(pid: PID) {
    let pid: u64 = pid.into();
    unsafe { core::arch::asm!("mov gs:9, {}", in(reg) pid) };
}

pub fn get_task_mgr_current_tid() -> TID {
    let pid: u64;
    unsafe { core::arch::asm!("mov {}, gs:17", lateout(reg) pid) };
    pid.into()
}

pub fn set_task_mgr_current_tid(tid: TID) {
    let tid: u64 = tid.into();
    unsafe { core::arch::asm!("mov gs:17, {}", in(reg) tid) };
}

pub fn get_task_mgr_current_ticks() -> u8 {
    let ticks: u16;
    unsafe { core::arch::asm!("mov {:e}, gs:25", lateout(reg) ticks) };
    ticks as u8
}

pub fn set_task_mgr_current_ticks(ticks: u8) {
    unsafe { core::arch::asm!("mov gs:25, {:e}", in(reg) ticks as u16) };
}

pub fn is_task_mgr_schedule() -> bool {
    let ticks: u16;
    unsafe { core::arch::asm!("mov {:e}, gs:29", lateout(reg) ticks) };
    // println!("ticks: {ticks}");
    ticks != 0
    // true
}

pub fn set_is_task_mgr_schedule(ticks: bool) {
    unsafe { core::arch::asm!("mov gs:29, {:e}", in(reg) ticks as u16) };
}
