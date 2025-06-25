use core::{
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};

use kernel_sys::types::VMMapFlags;
use x86_64::instructions::interrupts;

use crate::{
    gdt::CPULocalGDT,
    paging::{
        MemoryLoc, PER_CPU_MAP, PageAllocator, page::Page, page_allocator::global_allocator,
        page_table::Mapper, virt_addr_for_phys,
    },
    scheduling::process::{Thread, ThreadSched},
    syscall::syscall_kernel_handler,
};

// Do we probe the task manager for a new task?
pub static LS_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_ls_enabled() {
    LS_ENABLED.store(true, Ordering::Relaxed)
}

pub fn is_ls_enabled() -> bool {
    LS_ENABLED.load(Ordering::Relaxed)
}

#[repr(C)]
pub struct CPULocalStorage {
    // pinned locations
    pub stack_top: u64,
    pub kernel_syscall_entry: usize,

    pub core_id: u8,
    pub current_context: u8,
    pub scratch_stack_top: u64,
    pub current_task_ptr: u64,
    pub current_task_kernel_stack_top: u64,
    pub sched_task_sp: u64,
    pub sched_task_ip: u64,
    pub hold_interrupts_initial: u8,
    pub hold_interrupts_depth: u64,
    pub gdt_pointer: usize,
    // at 0x1000 (1 page down is GDT)
}

// Ensure that locations directly accessed outside kernel are in the expected location
const _: () = assert!(core::mem::offset_of!(CPULocalStorage, stack_top) == 0);
const _: () = assert!(core::mem::offset_of!(CPULocalStorage, kernel_syscall_entry) == 8);

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
    pub fn hold_interrupts_depth() -> u64 {
        unsafe { localstorage_read_imm!(hold_interrupts_depth: u64) }
    }

    #[inline]
    pub fn set_hold_interrupts_depth(val: u64) {
        unsafe { localstorage_write!(val => hold_interrupts_depth: u64) }
    }

    #[inline]
    pub fn hold_interrupts_initial() -> bool {
        unsafe { localstorage_read_imm!(hold_interrupts_initial: bool) }
    }

    #[inline]
    pub fn set_hold_interrupts_initial(val: bool) {
        unsafe { localstorage_write!(val as u8 => hold_interrupts_initial: u8) }
    }

    pub unsafe fn inc_hold_interrupts() {
        let depth = Self::hold_interrupts_depth();

        // time to disable and save interrupts state
        if depth == 0 {
            let enabled = interrupts::are_enabled();

            if enabled {
                interrupts::disable();
            }

            Self::set_hold_interrupts_initial(enabled);
        }

        Self::set_hold_interrupts_depth(depth + 1);
    }

    pub unsafe fn dec_hold_interrupts() {
        let depth = Self::hold_interrupts_depth() - 1;
        Self::set_hold_interrupts_depth(depth);

        // reset interrupt state
        if depth == 0 {
            let enabled = Self::hold_interrupts_initial();
            if enabled {
                interrupts::enable();
            }
        }
    }

    pub unsafe fn get_current_task<'l>() -> &'l Thread {
        unsafe {
            let ptr = localstorage_read_imm!(current_task_ptr: u64);

            assert_ne!(ptr, 0);
            &*(ptr as *const Thread)
        }
    }

    pub fn clear_current_task() {
        unsafe {
            let ptr = localstorage_read_imm!(current_task_ptr: u64);
            assert_ne!(ptr, 0);
            localstorage_write!(0 => current_task_ptr: u64);
        }
    }

    pub fn set_current_task(task: &Thread, sched: &ThreadSched) {
        unsafe {
            let old_ptr = localstorage_read_imm!(current_task_ptr: u64);
            assert_eq!(old_ptr, 0);

            let kstack_top = sched.kstack_top.as_u64();
            localstorage_write!(kstack_top => current_task_kernel_stack_top: u64);
            localstorage_write!(task as *const Thread as u64 => current_task_ptr: u64);
        }
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

    let alloc = global_allocator();
    for page in (vaddr_base..vaddr_base + gdt_size + 0xfff).step_by(0x1000) {
        let phys = alloc.allocate_page().unwrap();
        PER_CPU_MAP
            .lock()
            .map(alloc, Page::new(page), phys, VMMapFlags::WRITEABLE)
            .unwrap()
            .flush();
    }

    let ls = unsafe { &mut *(vaddr_base as *mut CPULocalStorage) };
    ls.core_id = core_id;
    ls.hold_interrupts_depth = 1; // to be decremented to 0 in `core_start_multitasking`
    ls.hold_interrupts_initial = 0;
    ls.gdt_pointer = (vaddr_base + 0x1000) as usize;
    ls.current_context = 0;
    ls.current_task_kernel_stack_top = 0;
    ls.current_task_ptr = 0;
    ls.kernel_syscall_entry = syscall_kernel_handler as usize;

    unsafe { crate::gdt::create_gdt_for_core(&mut *((vaddr_base + 0x1000) as *mut CPULocalGDT)) };

    vaddr_base
}

pub unsafe fn init_bsp_localstorage() {
    let gs_base = unsafe { new_cpu(0) };

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

    set_ls_enabled();
}

pub const CPU_STACK_SIZE_PAGES: usize = 10;

pub unsafe fn new_cpu(core_id: u8) -> u64 {
    let vaddr = unsafe { init_core(core_id) };
    let ls = unsafe { &mut *(vaddr as *mut CPULocalStorage) };

    let stack_base = global_allocator()
        .allocate_pages(CPU_STACK_SIZE_PAGES)
        .unwrap()
        .get_address();

    ls.stack_top = virt_addr_for_phys(stack_base) + CPU_STACK_SIZE_PAGES as u64 * 0x1000;

    let stack_base = global_allocator()
        .allocate_pages(CPU_STACK_SIZE_PAGES)
        .unwrap()
        .get_address();

    ls.scratch_stack_top = virt_addr_for_phys(stack_base) + CPU_STACK_SIZE_PAGES as u64 * 0x1000;
    vaddr
}
