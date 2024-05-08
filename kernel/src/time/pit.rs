use core::{
    arch::x86_64::_mm_pause,
    sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
};

use x86_64::{
    instructions::{
        interrupts::{self, without_interrupts},
        port::{Port, PortWriteOnly},
    },
    structures::idt::InterruptStackFrame,
};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    scheduling::taskmanager::{self, append_task_queue},
    wrap_function_registers,
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

wrap_function_registers!(tick => tick_handler);

unsafe extern "C" fn tick(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    if CPULocalStorageRW::get_core_id() == 0 {
        // Get the amount of milliseconds per interrupt
        let freq = 1000 / get_frequency();
        // Increment the uptime counter
        let uptime = TIME_SINCE_BOOT.fetch_add(freq, Ordering::Relaxed) + freq;
        // If we have a sleep target, wake up the job
        if uptime >= SLEEP_TARGET.load(Ordering::Relaxed) {
            // if over the target, try waking up processes
            SLEPT_PROCESSES.try_lock().map(|mut procs| {
                procs.retain(|&req_time, el| {
                    if req_time <= uptime {
                        append_task_queue(el);
                        false
                    } else {
                        true
                    }
                });
                SLEEP_TARGET.store(
                    procs.first_key_value().map(|(&k, _)| k).unwrap_or(u64::MAX),
                    core::sync::atomic::Ordering::Relaxed,
                )
            });
        }
    }

    // If timer is used for switching tasks switch task
    if SWITCH_TASK.load(Ordering::Acquire) {
        // match get_task_mgr_current_ticks().checked_sub(1) {
        //     Some(n) => set_task_mgr_current_ticks(n),
        //     None => {
        if !CPULocalStorageRW::get_stay_scheduled() {
            taskmanager::switch_task(stack_frame, regs);
        }
        //     }
        // }
    }
    // Ack interrupt
    unsafe { *(0xfee000b0 as *mut u32) = 0 }
}
