use core::{mem::MaybeUninit, u64};

use alloc::{sync::Arc, vec::Vec};
use conquer_once::spin::Lazy;
use kernel_userspace::{
    channel::{
        channel_create_rs, channel_read_rs, channel_read_val, channel_write_rs, ChannelReadResult,
    },
    object::KernelReference,
    port::{PortNotification, PortNotificationType},
    process::publish_handle,
    syscall::spawn_thread,
    INT_KB, INT_MOUSE, INT_PCI,
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

pub mod exceptions;
// pub mod hardware;
pub mod pic;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    kassert, lapic,
    mutex::Spinlock,
    port::KPort,
    scheduling::{
        process::{Thread, ThreadState},
        taskmanager::enter_sched,
        with_held_interrupts,
    },
    syscall::{self, SyscallError},
    time::uptime,
};

use self::pic::disable_pic;

// Unusable interrupt vectors
// 0..32 = Exceptions
// 32..48 = PIC Possible spurrius interrupts
const IRQ_OFFSET: usize = 49;
const LAPIC_INT: usize = 60;

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

pub static IDT: Lazy<Spinlock<InterruptDescriptorTable>> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    // Set idt table
    exceptions::set_exceptions_idt(&mut idt);
    // hardware::set_hardware_idt(&mut idt);
    pic::set_spurious_interrupts(&mut idt);
    syscall::set_syscall_idt(&mut idt);

    Spinlock::new(idt)
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
    unsafe {
        let i = IDT.lock();
        i.load_unsafe();
        disable_pic();
    };

    IDT.lock()[LAPIC_INT].set_handler_fn(lapic::tick_handler);
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

#[inline(always)]
fn int_interrupt_handler(vector: usize) {
    INTERRUPT_SOURCES[vector]
        .lock()
        .iter()
        .for_each(|e| e.trigger());
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

static INTERRUPT_SOURCES: Lazy<[Arc<Spinlock<Vec<Arc<KInterruptHandle>>>>; 3]> = Lazy::new(|| {
    [
        Arc::new(Default::default()),
        Arc::new(Default::default()),
        Arc::new(Default::default()),
    ]
});

/// Returns true if there were any interrupt events dispatched
pub fn check_interrupts() {
    let (service, sright) = channel_create_rs();
    publish_handle("INTERRUPTS", sright.id());

    let mut data = Vec::with_capacity(100);
    let mut handles = Vec::with_capacity(1);
    loop {
        match channel_read_rs(service.id(), &mut data, &mut handles) {
            ChannelReadResult::Ok => (),
            _ => todo!(),
        };
        let handle = KernelReference::from_id(handles[0]);
        spawn_thread({
            move || {
                let mut handles = Vec::with_capacity(1);
                loop {
                    let mut val = MaybeUninit::<usize>::uninit();
                    match channel_read_val(handle.id(), &mut val, &mut handles) {
                        ChannelReadResult::Ok => (),
                        ChannelReadResult::Size | ChannelReadResult::Closed => return,
                        _ => todo!(),
                    };

                    let req = unsafe { val.assume_init() };

                    if req > 2 {
                        error!("INTERRUPTS service got invalid id");
                        return;
                    }

                    let h = Arc::new(KInterruptHandle::new());

                    let id = with_held_interrupts(|| unsafe {
                        let thread = CPULocalStorageRW::get_current_task();
                        KernelReference::from_id(thread.process().add_value(h.clone().into()))
                    });

                    INTERRUPT_SOURCES[req].lock().push(h);

                    channel_write_rs(handle.id(), &[], &[id.id()]);
                }
            }
        });
    }
}

pub struct KInterruptHandle {
    inner: Spinlock<KInterruptHandleInner>,
}

struct KInterruptHandleInner {
    // has the last trigger been acked
    waiting_ack: bool,
    // do we have a waiting event (do not deliver if waiting_ack)
    pending: bool,
    waiter: InterruptWaiter,
}

enum InterruptWaiter {
    None,
    Thread(Arc<Thread>),
    Port { port: Arc<KPort>, key: u64 },
}

impl KInterruptHandle {
    pub const fn new() -> Self {
        Self {
            inner: Spinlock::new(KInterruptHandleInner {
                waiting_ack: false,
                pending: false,
                waiter: InterruptWaiter::None,
            }),
        }
    }

    pub fn trigger(&self) {
        let mut this = self.inner.lock();

        if this.pending || this.waiting_ack {
            this.pending = true;
            return;
        }

        match &this.waiter {
            InterruptWaiter::None => this.pending = true,
            InterruptWaiter::Thread(t) => {
                t.wake();
                this.pending = true;
                this.waiter = InterruptWaiter::None
            }
            InterruptWaiter::Port { port, key } => {
                port.notify(PortNotification {
                    key: *key,
                    ty: PortNotificationType::Interrupt {
                        timestamp: uptime(),
                    },
                });
            }
        }
    }

    pub fn wait(&self) -> Result<usize, SyscallError> {
        loop {
            let mut this = self.inner.lock();

            kassert!(matches!(this.waiter, InterruptWaiter::None));

            this.waiting_ack = false;

            if this.pending {
                this.pending = false;
                return Ok(0);
            }

            let thread = unsafe { CPULocalStorageRW::get_current_task() };
            let mut sched = thread.sched().lock();
            sched.state = ThreadState::Sleeping;
            this.waiter = InterruptWaiter::Thread(thread.thread());
            drop(this);
            enter_sched(&mut sched);
        }
    }

    pub fn set_port(&self, port: Arc<KPort>, key: u64) {
        let mut this = self.inner.lock();

        if this.pending && !this.waiting_ack {
            port.notify(PortNotification {
                key,
                ty: PortNotificationType::Interrupt {
                    timestamp: uptime(),
                },
            });
            this.waiting_ack = true;
        }

        this.waiter = InterruptWaiter::Port { port, key };
    }

    pub fn ack(&self) {
        let mut this = self.inner.lock();

        this.waiting_ack = false;

        if this.pending {
            match core::mem::replace(&mut this.waiter, InterruptWaiter::None) {
                InterruptWaiter::None => (),
                InterruptWaiter::Thread(t) => t.wake(),
                InterruptWaiter::Port { port, key } => {
                    port.notify(PortNotification {
                        key,
                        ty: PortNotificationType::Interrupt {
                            timestamp: uptime(),
                        },
                    });
                    this.waiter = InterruptWaiter::Port { port, key };
                }
            }

            this.waiting_ack = true;
        }
    }
}
