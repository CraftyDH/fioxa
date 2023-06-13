use core::sync::atomic::AtomicBool;

use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    service::{
        generate_tracking_number, SendServiceMessageDest, ServiceMessage, ServiceMessageType,
    },
    syscall::send_service_message,
};
use spin::mutex::Mutex;
use x86_64::{
    instructions::interrupts::without_interrupts,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

pub mod exceptions;
// pub mod hardware;
pub mod pic;

use lazy_static::lazy_static;

use crate::{
    gdt::TASK_SWITCH_INDEX,
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

impl Into<u8> for HardwareInterruptOffset {
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<usize> for HardwareInterruptOffset {
    fn into(self) -> usize {
        self as usize
    }
}

lazy_static! {
    pub static ref IDT: Mutex<InterruptDescriptorTable> = {
        let mut idt = InterruptDescriptorTable::new();
        // Set idt table
        exceptions::set_exceptions_idt(&mut idt);
        // hardware::set_hardware_idt(&mut idt);
        pic::set_spurious_interrupts(&mut idt);
        syscall::set_syscall_idt(&mut idt);

        Mutex::new(idt)
    };
}

#[macro_export]
macro_rules! interrupt_handler {
    ($fn: ident => $w:ident) => {
        pub extern "x86-interrupt" fn $w(i: InterruptStackFrame) {
            // let y: u16;
            // unsafe { core::arch::asm!("mov {0:x}, gs", out(reg) y) };
            // println!("Core: {y} received int");
            $fn(i);
            // Finish int
            unsafe { core::ptr::write_volatile(0xfee000B0 as *mut u32, 0) }
        }
    };
}

pub fn set_irq_handler(irq: usize, func: extern "x86-interrupt" fn(InterruptStackFrame)) {
    assert!(irq >= IRQ_OFFSET && irq <= 255);
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

static KB_INT: AtomicBool = AtomicBool::new(false);
static MOUSE_INT: AtomicBool = AtomicBool::new(false);
static PCI_INT: AtomicBool = AtomicBool::new(false);

interrupt_handler!(kb_interrupt_handler => keyboard_int_handler);
fn kb_interrupt_handler(_: InterruptStackFrame) {
    KB_INT.store(true, core::sync::atomic::Ordering::Relaxed)
}

interrupt_handler!(mouse_interrupt_handler => mouse_int_handler);
fn mouse_interrupt_handler(_: InterruptStackFrame) {
    MOUSE_INT.store(true, core::sync::atomic::Ordering::Relaxed)
}

interrupt_handler!(pci_interrupt_handler => pci_int_handler);
fn pci_interrupt_handler(_: InterruptStackFrame) {
    PCI_INT.store(true, core::sync::atomic::Ordering::Relaxed)
}

lazy_static! {
    pub static ref INTERRUPT_HANDLERS: [ServiceID; 3] = {
        let kb = service::new(ProcessID(0));
        let mouse = service::new(ProcessID(0));
        let pci = service::new(ProcessID(0));

        PUBLIC_SERVICES.lock().insert("INTERRUPTS:KB", kb);
        PUBLIC_SERVICES.lock().insert("INTERRUPTS:MOUSE", mouse);
        PUBLIC_SERVICES.lock().insert("INTERRUPTS:PCI", pci);

        [kb, mouse, pci]
    };
}

pub fn check_interrupts() -> bool {
    let mut res = false;
    if KB_INT.swap(false, core::sync::atomic::Ordering::Relaxed) {
        send_int_message(INTERRUPT_HANDLERS[0]);
        res = true;
    }
    if MOUSE_INT.swap(false, core::sync::atomic::Ordering::Relaxed) {
        send_int_message(INTERRUPT_HANDLERS[1]);
        res = true;
    }
    if PCI_INT.swap(false, core::sync::atomic::Ordering::Relaxed) {
        send_int_message(INTERRUPT_HANDLERS[2]);
        res = true;
    }
    res
}

fn send_int_message(service: ServiceID) {
    send_service_message(&ServiceMessage {
        service_id: service,
        sender_pid: ProcessID(0),
        tracking_number: generate_tracking_number(),
        destination: SendServiceMessageDest::ToSubscribers,
        message: ServiceMessageType::InterruptEvent,
    })
    .unwrap()
}
