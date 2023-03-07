use core::mem::size_of;

use crate::{
    gdt::CPULocalGDT,
    paging::{
        get_uefi_active_mapper,
        page_allocator::{frame_alloc_exec, request_page},
        page_table_manager::{page_4kb, Mapper},
        virt_addr_for_phys, MemoryLoc,
    },
};

#[repr(C, packed)]
pub struct CPULocalStorage {
    core_id: u8,
    stack_top: u64,
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
    unsafe { core::arch::asm!("mov {:e}, gs:0", out(reg) cid) };
    cid as u8
}
