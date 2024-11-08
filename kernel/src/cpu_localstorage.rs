use core::mem::size_of;

use alloc::boxed::Box;

use crate::{
    gdt::CPULocalGDT,
    paging::{
        get_uefi_active_mapper,
        page_allocator::{frame_alloc_exec, request_page},
        page_table_manager::{Mapper, Page},
        virt_addr_for_phys, MemoryLoc, MemoryMappingFlags,
    },
    scheduling::process::Thread,
};

#[repr(C, packed)]
pub struct CPULocalStorage {
    core_id: u8,
    stack_top: u64,
    current_context: u8,
    scratch_stack_top: u64,
    current_task_ptr: u64,
    current_task_kernel_stack_top: u64,
    sched_task_sp: u64,
    sched_task_ip: u64,
    // If non zero don't yield the task on timer interrupt
    stay_scheduled: u64,
    screen_redraw: u64,
    gdt_pointer: usize,
    // at 0x1000 (1 page down is GDT)
}

/// Reads the contents of the localstorage struct at offset $value, with given size
macro_rules! localstorage_read {
    ($value:tt => $res:ident: u8) =>  { core::arch::asm!("mov {0},   gs:{1}", lateout(reg_byte) $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($value:tt => $res:ident: u16) => { core::arch::asm!("mov {0:x}, gs:{1}", lateout(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($value:tt => $res:ident: u32) => { core::arch::asm!("mov {0:e}, gs:{1}", lateout(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($value:tt => $res:ident: u64) => { core::arch::asm!("mov {0:r}, gs:{1}", lateout(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
}

/// Writes the contents of the localstorage struct at offset $value, with given size
macro_rules! localstorage_write {
    // Convenience arm that makes use of the assumption that in rust a bool is a u8 with false = 0, true = 1
    ($res:expr => $value:tt: bool) => { localstorage_write!($res as u8 => $value: u8) };
    ($res:expr => $value:tt: u8)   => { core::arch::asm!("mov gs:{1}, {0}  ", in(reg_byte) $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($res:expr => $value:tt: u16)  => { core::arch::asm!("mov gs:{1}, {0:x}", in(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($res:expr => $value:tt: u32)  => { core::arch::asm!("mov gs:{1}, {0:e}", in(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
    ($res:expr => $value:tt: u64)  => { core::arch::asm!("mov gs:{1}, {0:r}", in(reg)      $res, const core::mem::offset_of!(CPULocalStorage, $value)) };
}

/// Creates a value, reads into it and returns the value
macro_rules! localstorage_read_imm {
    // Convenience arm that makes use of the assumption that in rust a bool is a u8 with false = 0, true = 1
    ($value:tt: bool) => {
        localstorage_read_imm!($value: u8) != 0
    };
    ($value:tt: $ty:tt) => {{
        let val: $ty;
        localstorage_read!($value => val:$ty);
        val
    }};
}

/// Struct that allows for reading CPULocalStorage runtime values (100% volatile)
pub struct CPULocalStorageRW {}

impl CPULocalStorageRW {
    #[inline]
    pub fn get_core_id() -> u8 {
        unsafe { localstorage_read_imm!(core_id: u8) }
    }

    #[inline]
    pub fn set_core_id(val: u8) {
        unsafe { localstorage_write!(val => core_id: u8) }
    }

    #[inline]
    pub fn get_stack_top() -> u64 {
        unsafe { localstorage_read_imm!(stack_top: u64) }
    }

    #[inline]
    pub fn get_stay_scheduled() -> u64 {
        unsafe { localstorage_read_imm!(stay_scheduled: u64) }
    }

    #[inline]
    pub unsafe fn inc_stay_scheduled() {
        core::arch::asm!(
            "inc qword ptr gs:{}",
            const core::mem::offset_of!(CPULocalStorage, stay_scheduled),
            options(nostack)
        );
    }

    #[inline]
    pub unsafe fn dec_stay_scheduled() {
        core::arch::asm!(
            "dec qword ptr gs:{}",
            const core::mem::offset_of!(CPULocalStorage, stay_scheduled),
            options(nostack)
        );
    }

    /// SAFTEY: The pointer must be dropped before the next take_current_task call, and there cannot be multiple pointers at once
    pub unsafe fn get_current_task<'l>() -> &'l mut Thread {
        unsafe {
            let ptr = localstorage_read_imm!(current_task_ptr: u64);

            assert_ne!(ptr, 0);
            &mut *(ptr as *mut Thread)
        }
    }

    pub fn take_current_task() -> Box<Thread> {
        unsafe {
            let ptr = localstorage_read_imm!(current_task_ptr: u64);
            localstorage_write!(0 => current_task_ptr: u64);

            assert_ne!(ptr, 0);
            Box::from_raw(ptr as *mut _)
        }
    }

    pub fn set_current_task(task: Box<Thread>) {
        unsafe {
            let old_ptr = localstorage_read_imm!(current_task_ptr: u64);
            assert_eq!(old_ptr, 0);

            let kstack_top = task.kstack_top.as_u64();
            let ptr = Box::into_raw(task);
            localstorage_write!(kstack_top => current_task_kernel_stack_top: u64);
            localstorage_write!(ptr => current_task_ptr: u64);
        }
    }

    #[inline]
    pub fn get_screen_redraw_time() -> u64 {
        unsafe { localstorage_read_imm!(screen_redraw: u64) }
    }

    pub fn set_screen_redraw_time(t: u64) {
        unsafe { localstorage_write!(t => screen_redraw: u64) }
    }

    pub fn get_gdt() -> &'static mut CPULocalGDT {
        unsafe { &mut *(localstorage_read_imm!(gdt_pointer: u64) as *mut CPULocalGDT) }
    }

    pub fn get_context() -> u8 {
        unsafe { localstorage_read_imm!(current_context: u8) }
    }
}

pub unsafe fn init_core(core_id: u8) -> u64 {
    let vaddr_base = MemoryLoc::PerCpuMem as u64 + 0x100_0000 * core_id as u64;

    let cpu_lsize = size_of::<CPULocalStorage>() as u64;
    let gdt_size = size_of::<CPULocalGDT>() as u64;
    assert!(cpu_lsize <= 0x1000);
    // Amount of cpu storage we have
    assert!(gdt_size + 0x1000 <= 0x10_0000);

    let mut map = get_uefi_active_mapper();

    for page in (vaddr_base..vaddr_base + gdt_size + 0xfff).step_by(0x1000) {
        let phys = request_page().unwrap().leak();
        map.map_memory(Page::new(page), phys, MemoryMappingFlags::WRITEABLE)
            .unwrap()
            .flush();
    }

    let ls = unsafe { &mut *(vaddr_base as *mut CPULocalStorage) };
    ls.core_id = core_id;
    ls.stay_scheduled = 1; // to be decremented to 0 in `core_start_multitasking`
    ls.gdt_pointer = (vaddr_base + 0x1000) as usize;
    ls.current_context = 0;
    ls.current_task_kernel_stack_top = 0;

    ls.current_task_ptr = 0;

    crate::gdt::create_gdt_for_core(unsafe { &mut *((vaddr_base + 0x1000) as *mut CPULocalGDT) });

    vaddr_base
}

pub unsafe fn init_bsp_task() {
    let gs_base = new_cpu(0);

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

pub const CPU_STACK_SIZE_PAGES: u64 = 10;

pub unsafe fn new_cpu(core_id: u8) -> u64 {
    let vaddr = init_core(core_id);
    let ls = unsafe { &mut *(vaddr as *mut CPULocalStorage) };

    let stack_base = frame_alloc_exec(|c| c.request_cont_pages(CPU_STACK_SIZE_PAGES as usize))
        .unwrap()
        .next()
        .unwrap()
        .leak()
        .get_address();

    ls.stack_top = virt_addr_for_phys(stack_base) + CPU_STACK_SIZE_PAGES * 0x1000;

    let stack_base = frame_alloc_exec(|c| c.request_cont_pages(CPU_STACK_SIZE_PAGES as usize))
        .unwrap()
        .next()
        .unwrap()
        .leak()
        .get_address();

    ls.scratch_stack_top = virt_addr_for_phys(stack_base) + CPU_STACK_SIZE_PAGES * 0x1000;
    vaddr
}
