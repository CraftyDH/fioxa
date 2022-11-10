use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};

use x86_64::{
    instructions::{interrupts::without_interrupts, port::Port},
    structures::idt::InterruptStackFrame,
};

use crate::{
    assembly::registers::Registers, scheduling::taskmanager::TASKMANAGER, syscall::yield_now,
    wrap_function_registers,
};

const PIT_BASE_FREQUENCY: usize = 1193182;

/// This is measured in milliseconds
static TIME_SINCE_BOOT: AtomicUsize = AtomicUsize::new(0);
static PIT_DIVISOR: AtomicU16 = AtomicU16::new(65535);

// Do we probe the task manager for a new task?
static SWITCH_TASK: AtomicBool = AtomicBool::new(false);

pub fn set_divisor(mut divisor: u16) {
    // Prevent ridicuosly fast interrupts
    // Max at 1ms
    if divisor < (PIT_BASE_FREQUENCY / 1000) as u16 {
        divisor = (PIT_BASE_FREQUENCY / 1000) as u16
    }

    without_interrupts(|| {
        PIT_DIVISOR.store(divisor, Ordering::Release);

        let mut cmd: Port<u8> = Port::new(0x43);
        let mut data: Port<u8> = Port::new(0x40);

        unsafe {
            cmd.write(0b00_11_011_0);
            // Write first 8 bits
            data.write((divisor & 0xFF) as u8);
            // Write upper 8 bits
            data.write((divisor & 0xFF00 >> 8) as u8);
        }
    })
}

pub fn get_frequency() -> usize {
    return PIT_BASE_FREQUENCY / PIT_DIVISOR.load(Ordering::Acquire) as usize;
}

pub fn set_frequency(frequency: usize) {
    let mut divisor = PIT_BASE_FREQUENCY / frequency;
    // Slowest divisor is 65535
    if divisor > 65535 {
        divisor = 65535
    }
    set_divisor(divisor as u16);
}

// Returns system uptime in milliseconds
pub fn get_uptime() -> usize {
    TIME_SINCE_BOOT.load(Ordering::Acquire)
}

pub fn sleep(ms: usize) {
    let end_time = get_uptime() + ms;
    while get_uptime() < end_time {
        // Let next process have a go
        yield_now();
    }
}

pub fn start_switching_tasks() {
    SWITCH_TASK.store(true, Ordering::Release)
}

pub fn stop_switching_tasks() {
    SWITCH_TASK.store(false, Ordering::Release)
}

pub fn get_switching_status() -> bool {
    SWITCH_TASK.load(Ordering::Relaxed)
}

wrap_function_registers!(tick => tick_handler);

extern "C" fn tick(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Get the amount of milliseconds per interrupt
    // let freq = 1000 / get_frequency();
    // Increment the uptime counter
    // TIME_SINCE_BOOT.fetch_add(freq, Ordering::Release);

    // If timer is used for switching tasks switch task
    if SWITCH_TASK.load(Ordering::Acquire) {
        TASKMANAGER
            .try_lock()
            .and_then(|mut t| Some(t.switch_task(stack_frame, regs)));
    }
    // Ack interrupt
    unsafe { *(0xfee000b0 as *mut u32) = 0 }
}
