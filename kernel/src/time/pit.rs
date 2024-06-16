use core::{
    arch::x86_64::_mm_pause,
    sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
};

use alloc::boxed::Box;
use x86_64::{
    instructions::{
        interrupts::{self, without_interrupts},
        port::{Port, PortWriteOnly},
    },
    structures::idt::InterruptStackFrame,
};

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    scheduling::{
        process::ThreadStatus,
        taskmanager::{context_switch_helper, get_next_task, push_task_queue, queue_task_callback},
    },
    screen::gop::WRITER,
};

use super::{SLEEP_TARGET, SLEPT_PROCESSES};

const PIT_BASE_FREQUENCY: u64 = 1193182;

/// This is measured in milliseconds
static TIME_SINCE_BOOT: AtomicU64 = AtomicU64::new(0);
static PIT_DIVISOR: AtomicU16 = AtomicU16::new(65535);

// Do we probe the task manager for a new task?
static SWITCH_TASK: AtomicBool = AtomicBool::new(false);

pub struct ProgrammableIntervalTimer {
    data: Port<u8>,
    cmd: PortWriteOnly<u8>,
}

impl ProgrammableIntervalTimer {
    pub const fn new() -> Self {
        Self {
            data: Port::new(0x40),
            cmd: PortWriteOnly::new(0x43),
        }
    }

    pub fn set_frequency(&mut self, freq: u64) {
        let mut divisor = PIT_BASE_FREQUENCY / freq;
        // Slowest divisor is 65535
        if divisor > 65535 {
            divisor = 65535
        }
        self.set_divisor(divisor as u16)
    }

    pub fn set_divisor(&mut self, divisor: u16) {
        without_interrupts(|| {
            PIT_DIVISOR.store(divisor, Ordering::Release);

            unsafe {
                // Rate generator
                self.cmd.write(0b00_11_010_0);
                // Write first 8 bits
                self.data.write((divisor & 0xFF) as u8);
                // Write upper 8 bits
                self.data.write((divisor & 0xFF00 >> 8) as u8);
            }
        })
    }

    pub fn spin_ms(&self, time: u64) {
        assert!(
            interrupts::are_enabled(),
            "Spin sleep ms on PIC was called, but interrupts are disabled"
        );

        let end_time = get_uptime() + time;
        while get_uptime() < end_time {
            // Let next process have a go
            unsafe {
                _mm_pause();
            }
        }
    }
}

// Returns system uptime in milliseconds
pub fn get_uptime() -> u64 {
    TIME_SINCE_BOOT.load(Ordering::Acquire)
}

pub fn get_frequency() -> u64 {
    PIT_BASE_FREQUENCY / PIT_DIVISOR.load(Ordering::Acquire) as u64
}

pub fn start_switching_tasks() {
    SWITCH_TASK.store(true, Ordering::Relaxed)
}

pub fn stop_switching_tasks() {
    SWITCH_TASK.store(false, Ordering::Relaxed)
}

pub fn is_switching_tasks() -> bool {
    SWITCH_TASK.load(Ordering::Relaxed)
}

#[naked]
pub extern "x86-interrupt" fn tick_handler(_: InterruptStackFrame) {
    unsafe {
        core::arch::asm!(
            "push rbp",
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "lea rdi, [rip+2f]",
            "mov rsi, rsp",
            "mov rbx, rsi", // save the rsp in a preserved register
            "mov rsp, gs:0xA", // load cpu stack
            "xor eax, eax",
            "mov gs:0x9, al", // set cpu context to 0
            "call {}",
            // we didn't context switch restore stack
            "mov rsp, rbx",
            // come back from context switch
            "2:",
            "mov al, 2",
            "mov gs:0x9, al", // set cpu context
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop r11",
            "pop r10",
            "pop r9",
            "pop r8",
            "pop rdi",
            "pop rsi",
            "pop rdx",
            "pop rcx",
            "pop rbx",
            "pop rax",
            "pop rbp",
            "iretq",
            sym tick,
            options(noreturn)
        );
    }
}

unsafe extern "C" fn tick(saved_ip: usize, saved_rsp: usize) {
    let stay_scheduled = CPULocalStorageRW::get_stay_scheduled();

    if CPULocalStorageRW::get_core_id() == 0 {
        // Get the amount of milliseconds per interrupt
        let freq = 1000 / get_frequency() / 2;
        // Increment the uptime counter
        let uptime = TIME_SINCE_BOOT.fetch_add(freq, Ordering::Relaxed) + freq;
        // If we have a sleep target, wake up the job
        if !stay_scheduled && uptime >= SLEEP_TARGET.load(Ordering::Relaxed) {
            // if over the target, try waking up processes
            if let Some(mut procs) = SLEPT_PROCESSES.try_lock() {
                procs.retain(|&req_time, el| {
                    if req_time <= uptime {
                        while let Some(e) = el.pop() {
                            if let Some(handle) = e.upgrade() {
                                let status = &mut *handle.status.lock();
                                match core::mem::take(status) {
                                    ThreadStatus::Ok | ThreadStatus::BlockingRet(_) => {
                                        panic!("bad state")
                                    }
                                    ThreadStatus::Blocking => {
                                        *status = ThreadStatus::BlockingRet(0)
                                    }
                                    ThreadStatus::Blocked(thread) => push_task_queue(thread),
                                }
                            }
                        }
                        false
                    } else {
                        true
                    }
                });
                SLEEP_TARGET.store(
                    procs.first_key_value().map(|(&k, _)| k).unwrap_or(u64::MAX),
                    core::sync::atomic::Ordering::Relaxed,
                )
            }
        }

        // potentially update screen every 16ms
        //* Very important that CPU doesn't have the stay scheduled flag (deadlock possible otherwise)
        // TODO: Can we VSYNC this? Could stop the tearing.
        if !stay_scheduled && uptime > CPULocalStorageRW::get_screen_redraw_time() {
            CPULocalStorageRW::set_screen_redraw_time(uptime + 16);
            let mut w = WRITER.get().unwrap().lock();
            w.redraw_if_needed();
        }
    }

    unsafe {
        // Ack interrupt
        *(0xfee000b0 as *mut u32) = 0;

        // switch task if possible
        if !stay_scheduled && SWITCH_TASK.load(Ordering::Relaxed) {
            if let Some(t) = get_next_task() {
                context_switch_helper(Box::into_raw(t), queue_task_callback, saved_rsp, saved_ip);
            }
        }
    }
}
