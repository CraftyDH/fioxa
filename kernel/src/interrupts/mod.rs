use core::sync::atomic::AtomicU8;

use alloc::vec::Vec;
use conquer_once::spin::Lazy;
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    service::{
        generate_tracking_number, SendServiceMessageDest, ServiceMessage, ServiceMessageType,
    },
    syscall::send_service_message,
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
    cpu_localstorage::CPULocalStorageRW,
    gdt::TASK_SWITCH_INDEX,
    scheduling::taskmanager::enter_core_mgmt,
    service::{self, PUBLIC_SERVICES},
    syscall,
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
    println!("IPI {:?}", s)
}

interrupt_handler!(spurious => spurious_handler);

pub fn spurious(s: InterruptStackFrame) {
    println!("Spurious {:?}", s)
}

// wrap_function_registers!(task_switch => task_switch_handler);

// pub fn task_switch(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
//     print!("$");
//     without_interrupts(|| {
//         taskmanager::switch_task(stack_frame, regs);
//         unsafe { core::ptr::write_volatile(0xfee000B0 as *mut u32, 0) }
//     })
// }

/// We pack the interrupts into an atomic because that reduces atomic contention in the highly polled check_interrupts.
static INT_VEC: AtomicU8 = AtomicU8::new(0);

const KB_INT: u8 = 0;
const MOUSE_INT: u8 = 1;
const PCI_INT: u8 = 2;

#[inline(always)]
fn int_interrupt_handler(vector: u8) {
    INT_VEC.fetch_or(1 << vector, core::sync::atomic::Ordering::Relaxed);
    enter_core_mgmt();
}

interrupt_handler!(kb_interrupt_handler => keyboard_int_handler);
fn kb_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(KB_INT)
}

interrupt_handler!(mouse_interrupt_handler => mouse_int_handler);
fn mouse_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(MOUSE_INT)
}

interrupt_handler!(pci_interrupt_handler => pci_int_handler);
fn pci_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(PCI_INT)
}

pub static INTERRUPT_HANDLERS: Lazy<[ServiceID; 3]> = Lazy::new(|| {
    let kb = service::new(ProcessID(0));
    let mouse = service::new(ProcessID(0));
    let pci = service::new(ProcessID(0));

    PUBLIC_SERVICES.lock().insert("INTERRUPTS:KB".into(), kb);
    PUBLIC_SERVICES
        .lock()
        .insert("INTERRUPTS:MOUSE".into(), mouse);
    PUBLIC_SERVICES.lock().insert("INTERRUPTS:PCI".into(), pci);

    [kb, mouse, pci]
});

/// Returns true if there were any interrupt events dispatched
pub fn check_interrupts(send_buffer: &mut Vec<u8>) -> bool {
    let interrupts = INT_VEC.swap(0, core::sync::atomic::Ordering::Relaxed);
    let handlers = INTERRUPT_HANDLERS.as_ref();
    if interrupts & (1 << KB_INT) > 0 {
        send_int_message(handlers[KB_INT as usize], send_buffer);
    }
    if interrupts & (1 << MOUSE_INT) > 0 {
        send_int_message(handlers[MOUSE_INT as usize], send_buffer)
    }
    if interrupts & (1 << PCI_INT) > 0 {
        send_int_message(handlers[KB_INT as usize], send_buffer);
    }
    // check if at least 1 interrupt occured
    interrupts != 0
}

fn send_int_message(service: ServiceID, send_buffer: &mut Vec<u8>) {
    send_service_message(
        &ServiceMessage {
            service_id: service,
            sender_pid: CPULocalStorageRW::get_current_pid(),
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToSubscribers,
            message: ServiceMessageType::InterruptEvent,
        },
        send_buffer,
    )
    .unwrap()
}
