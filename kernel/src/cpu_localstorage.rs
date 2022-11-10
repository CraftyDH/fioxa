use crate::paging::{
    page_allocator::{frame_alloc_exec, request_page},
    page_table_manager::ident_map_curr_process,
};

#[repr(C, packed)]
pub struct CPULocalStorage {
    core_id: u8,
    stack_top: u64,
}

pub unsafe fn init_bsp_task() {
    let local_storage = request_page().unwrap();
    let ls = unsafe { &mut *(local_storage as *mut CPULocalStorage) };
    ls.core_id = 0;

    unsafe {
        core::arch::asm!(
            "
            mov edx, 0
            mov ecx, 0xC0000101
            wrmsr
            mov ecx, 0xC0000102
            wrmsr
            ", in("eax") local_storage, lateout("edx") _,  lateout("ecx") _
        )
    }
}

pub const CPU_STACK_SIZE: u64 = 0x1000 * 10;

pub unsafe fn new_cpu(core_id: u8) -> u64 {
    let local_storage = request_page().unwrap();
    ident_map_curr_process(local_storage, true);
    let ls = unsafe { &mut *(local_storage as *mut CPULocalStorage) };
    ls.core_id = core_id;

    let stack_base = frame_alloc_exec(|c| c.lock().request_cont_pages(10)).unwrap();

    // TODO: Fix page allocator to give continuous pages instead of this hack

    ls.stack_top = stack_base + CPU_STACK_SIZE;
    local_storage
}
pub fn get_current_cpu_id() -> u8 {
    let pid: u16;
    unsafe { core::arch::asm!("mov {:e}, gs:0", out(reg) pid) };
    pid as u8
}
