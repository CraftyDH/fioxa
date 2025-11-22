use core::{hint::spin_loop, sync::atomic::AtomicI32};

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_sys::{
    syscall::sys_process_spawn_thread,
    types::{SysPortNotification, SysPortNotificationValue, SyscallResult},
};
use kernel_userspace::{
    handle::Handle,
    interrupt::{Interrupt, InterruptVector, InterruptsServiceExecutor, InterruptsServiceImpl},
    ipc::IPCChannel,
    service::ServiceExecutor,
};
use spin::Lazy;
use x86_64::{
    instructions::{hlt, interrupts},
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

pub mod exceptions;
// pub mod hardware;
pub mod pic;

use crate::{
    boot_aps::LAPIC_IDS,
    cpu_localstorage::CPULocalStorageRW,
    ioapic::{BOOT_BSP_ID, get_current_core_id, send_ipi_to},
    kassert,
    lapic::{self, disable_localapic},
    mutex::Spinlock,
    port::KPort,
    scheduling::{
        process::{Thread, ThreadState},
        taskmanager::{enter_sched, reset_sse},
        with_held_interrupts,
    },
    syscall,
    time::uptime,
};

use self::pic::disable_pic;

// Unusable interrupt vectors
// 0..32 = Exceptions
// 32..48 = PIC Possible spurrius interrupts
const IRQ_OFFSET: u8 = 49;
const LAPIC_INT: u8 = 60;

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
            $fn(i);
            // Finish int
            unsafe { core::ptr::write_volatile(($crate::lapic::LAPIC_ADDR + 0xb0) as *mut u32, 0) }
        }
    };
}

pub fn set_irq_handler(irq: u8, func: extern "x86-interrupt" fn(InterruptStackFrame)) {
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
    set_irq_handler(0xFE, kexec_handler);
    set_irq_handler(0xFF, spurious_handler);
}

interrupt_handler!(ipi_handler => ipi_interrupt_handler);

pub fn ipi_handler(s: InterruptStackFrame) {
    info!("IPI {s:?}")
}

interrupt_handler!(spurious => spurious_handler);

pub fn spurious(s: InterruptStackFrame) {
    debug!("Spurious {s:?}")
}

pub static KEXEC_COUNT: AtomicI32 = AtomicI32::new(-1);
pub static mut KEXEC_FN: Option<Box<dyn FnOnce() + Send + Sync>> = None;

interrupt_handler!(kevec_int => kexec_handler);

fn kevec_int(_: InterruptStackFrame) {
    unsafe {
        interrupts::disable();
        core::ptr::write_volatile((crate::lapic::LAPIC_ADDR + 0xb0) as *mut u32, 0);
        perform_kexec_handler()
    };
}

pub unsafe fn perform_kexec_handler() -> ! {
    unsafe { reset_sse() };

    let our_id = get_current_core_id();

    if BOOT_BSP_ID.get().is_some_and(|&id| id == our_id) {
        KEXEC_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let len = LAPIC_IDS.get().unwrap().len();

        let mut i = 0;
        loop {
            let c = KEXEC_COUNT.load(core::sync::atomic::Ordering::Acquire) as usize;
            if c == len {
                break;
            }
            i -= 1;
            if i <= 0 {
                i = 1000;
                info!("Waiting for cores to shutdown: {}/{}", c, len);
            }
            spin_loop();
        }

        unsafe {
            let f = core::ptr::replace(&raw mut KEXEC_FN, None);
            (f.unwrap())();
        }
    } else {
        unsafe {
            disable_localapic();
        }
        KEXEC_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    loop {
        hlt();
    }
}

pub fn execute_kexec(f: impl FnOnce() + Send + Sync + 'static) -> ! {
    interrupts::disable();
    if let Some(ids) = LAPIC_IDS.get() {
        let r = KEXEC_COUNT.compare_exchange(
            -1,
            0,
            core::sync::atomic::Ordering::Acquire,
            core::sync::atomic::Ordering::Relaxed,
        );
        // handle other core beat us
        if r.is_err() {
            interrupts::enable();
            loop {
                hlt();
            }
        }
        unsafe {
            core::ptr::replace(&raw mut KEXEC_FN, Some(Box::new(f)));
            let our_id = get_current_core_id();
            info!("kexec core id: {our_id}");
            for &id in ids {
                if id == our_id {
                    continue;
                }
                send_ipi_to(id, 0xFE);
            }
            perform_kexec_handler();
        }
    } else {
        // single core
        f();
        loop {
            hlt();
        }
    }
}

#[inline(always)]
fn int_interrupt_handler(vector: InterruptVector) {
    INTERRUPT_SOURCES[vector as usize]
        .lock()
        .iter()
        .for_each(|e| e.trigger());
}

interrupt_handler!(kb_interrupt_handler => keyboard_int_handler);
fn kb_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(InterruptVector::Keyboard)
}

interrupt_handler!(mouse_interrupt_handler => mouse_int_handler);
fn mouse_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(InterruptVector::Mouse)
}

interrupt_handler!(pci_interrupt_handler => pci_int_handler);
fn pci_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(InterruptVector::PCI)
}
interrupt_handler!(com1_interrupt_handler => com1_int_handler);
fn com1_interrupt_handler(_: InterruptStackFrame) {
    int_interrupt_handler(InterruptVector::COM1)
}

type InterruptSource = Arc<Spinlock<Vec<Arc<KInterruptHandle>>>>;
static INTERRUPT_SOURCES: Lazy<[InterruptSource; 4]> = Lazy::new(|| {
    [
        Arc::new(Default::default()),
        Arc::new(Default::default()),
        Arc::new(Default::default()),
        Arc::new(Default::default()),
    ]
});

struct InterruptService;

impl InterruptsServiceImpl for InterruptService {
    fn get_interrupt(
        &mut self,
        vector: kernel_userspace::interrupt::InterruptVector,
    ) -> Option<kernel_userspace::interrupt::Interrupt> {
        let h = Arc::new(KInterruptHandle::new());

        let id = with_held_interrupts(|| unsafe {
            let thread = CPULocalStorageRW::get_current_task();
            Handle::from_id(thread.process().add_value(h.clone().into()))
        });

        INTERRUPT_SOURCES.get(vector as usize)?.lock().push(h);
        Some(Interrupt::from_handle(id))
    }
}

/// Returns true if there were any interrupt events dispatched
pub fn check_interrupts() {
    ServiceExecutor::with_name("INTERRUPTS", |channel| {
        sys_process_spawn_thread({
            move || match InterruptsServiceExecutor::new(
                IPCChannel::from_channel(channel),
                InterruptService,
            )
            .run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
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

impl Default for KInterruptHandle {
    fn default() -> Self {
        Self::new()
    }
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
                port.notify(SysPortNotification {
                    key: *key,
                    value: SysPortNotificationValue::Interrupt {
                        timestamp: uptime(),
                    },
                });
            }
        }
    }

    pub fn wait(&self) -> SyscallResult {
        loop {
            let mut this = self.inner.lock();

            kassert!(matches!(this.waiter, InterruptWaiter::None));

            this.waiting_ack = false;

            if this.pending {
                this.pending = false;
                return Ok(());
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
            port.notify(SysPortNotification {
                key,
                value: SysPortNotificationValue::Interrupt {
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
                    port.notify(SysPortNotification {
                        key,
                        value: SysPortNotificationValue::Interrupt {
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
