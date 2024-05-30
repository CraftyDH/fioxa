use alloc::sync::Arc;
use conquer_once::spin::{Lazy, OnceCell};
use kernel_userspace::{
    message::MessageHandle,
    object::{KernelObjectType, KernelReference},
    service::deserialize,
    socket::{SocketListenHandle, SocketRecieveResult},
    syscall::spawn_thread,
    INT_KB, INT_MOUSE, INT_PCI,
};
use spin::Mutex;
use x86_64::{
    instructions::interrupts::without_interrupts,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

pub mod exceptions;
// pub mod hardware;
pub mod pic;

use crate::{
    cpu_localstorage::CPULocalStorageRW, event::KEvent, gdt::TASK_SWITCH_INDEX, syscall,
    time::pit::tick_handler,
};

use self::pic::disable_pic;

// Unusable interrupt vectors
// 0..32 = Exceptions
// 32..48 = PIC Possible spurrius interrupts
const IRQ_OFFSET: usize = 49;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
#[allow(dead_code)]
pub enum HardwareInterruptOffset {
    // PIC1
    Timer,
    Keyboard,
    Cascade,
    COM2,
    COM1,
    LPT2,
    Floppy,
    LPT1,
    // PIC2
    CMOSClock,
    Free1,
    Free2,
    Free3,
    Mouse,
    FPU,
    ATAPrimary,
    ATASecondary,
}

impl From<HardwareInterruptOffset> for u8 {
    fn from(val: HardwareInterruptOffset) -> Self {
        val as u8
    }
}

impl From<HardwareInterruptOffset> for usize {
    fn from(val: HardwareInterruptOffset) -> Self {
        val as usize
    }
}

pub static IDT: Lazy<Mutex<InterruptDescriptorTable>> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    // Set idt table
    exceptions::set_exceptions_idt(&mut idt);
    // hardware::set_hardware_idt(&mut idt);
    pic::set_spurious_interrupts(&mut idt);
    syscall::set_syscall_idt(&mut idt);

    Mutex::new(idt)
});

#[macro_export]
macro_rules! interrupt_handler {
    ($fn: ident => $w:ident) => {
        pub extern "x86-interrupt" fn $w(i: InterruptStackFrame) {
            // let y: u16;
            // unsafe { core::arch::asm!("mov {0:x}, gs", out(reg) y) };
            // println!("Core: {y} received int");
            $fn(i);
            // Finish int
            unsafe { core::ptr::write_volatile(0xfee000b0 as *mut u32, 0) }
        }
    };
}

pub fn set_irq_handler(irq: usize, func: extern "x86-interrupt" fn(InterruptStackFrame)) {
    assert!((IRQ_OFFSET..=255).contains(&irq));
    IDT.lock()[irq].set_handler_fn(func);
}

pub fn init_idt() {
    without_interrupts(|| {
        let i = IDT.lock();
        unsafe {
            i.load_unsafe();
            disable_pic();
        };
    });

    unsafe {
        IDT.lock()[IRQ_OFFSET]
            .set_handler_fn(tick_handler)
            .set_stack_index(TASK_SWITCH_INDEX);
    }
    // set_irq_handler(101, task_switch_handler);
    set_irq_handler(100, ipi_interrupt_handler);
    set_irq_handler(0xFF, spurious_handler);
}

interrupt_handler!(ipi_handler => ipi_interrupt_handler);

pub fn ipi_handler(s: InterruptStackFrame) {
    info!("IPI {:?}", s)
}

interrupt_handler!(spurious => spurious_handler);

pub fn spurious(s: InterruptStackFrame) {
    debug!("Spurious {:?}", s)
}

// wrap_function_registers!(task_switch => task_switch_handler);

// pub fn task_switch(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
//     print!("$");
//     without_interrupts(|| {
//         taskmanager::switch_task(stack_frame, regs);
//         unsafe { core::ptr::write_volatile(0xfee000B0 as *mut u32, 0) }
//     })
// }

#[inline(always)]
fn int_interrupt_handler(vector: usize) {
    INTERRUPT_SOURCES
        .get()
        .map(|e| e[vector].lock().trigger_edge(true));
}

interrupt_handler!(kb_interrupt_handler => keyboard_int_handler);
fn kb_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(INT_KB)
}

interrupt_handler!(mouse_interrupt_handler => mouse_int_handler);
fn mouse_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(INT_MOUSE)
}

interrupt_handler!(pci_interrupt_handler => pci_int_handler);
fn pci_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(INT_PCI)
}

static INTERRUPT_SOURCES: OnceCell<[Arc<Mutex<KEvent>>; 3]> = OnceCell::uninit();

/// Returns true if there were any interrupt events dispatched
pub fn check_interrupts() {
    let kb = KEvent::new();
    let mouse = KEvent::new();
    let pci = KEvent::new();
    INTERRUPT_SOURCES
        .try_init_once(|| [kb.clone(), mouse.clone(), pci.clone()])
        .unwrap();

    let ids = Arc::new(without_interrupts(|| unsafe {
        let thread = CPULocalStorageRW::get_current_task();
        [
            KernelReference::from_id(thread.process().add_value(kb.into())),
            KernelReference::from_id(thread.process().add_value(mouse.into())),
            KernelReference::from_id(thread.process().add_value(pci.into())),
        ]
    }));

    let service = SocketListenHandle::listen("INTERRUPTS").expect("we should be able to listen");

    loop {
        let conn = service.blocking_accept();
        spawn_thread({
            let ids = ids.clone();
            move || loop {
                match conn.blocking_recv() {
                    Ok((msg, ty)) => {
                        if ty != KernelObjectType::Message {
                            error!("INTERRUPTS service got invalid message");
                            return;
                        }
                        let msg = MessageHandle::from_kref(msg).read_vec();

                        let Ok(req) = deserialize::<usize>(&msg) else {
                            error!("INTERRUPTS service got invalid message desc");
                            return;
                        };

                        let Some(id) = ids.get(req) else {
                            error!("INTERRUPTS service got invalid id");
                            return;
                        };

                        let Ok(()) = conn.blocking_send(id) else {
                            error!("INTERRUPT service got eof");
                            return;
                        };
                    }
                    Err(SocketRecieveResult::EOF) => return,
                    Err(SocketRecieveResult::None) => break,
                }
            }
        });
    }
}
