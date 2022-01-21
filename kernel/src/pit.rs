use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};

use x86_64::{
    instructions::{interrupts::without_interrupts, port::Port},
    structures::idt::InterruptStackFrame,
};

use crate::{
    assembly::registers::Registers,
    interrupts::hardware::{PIC1_OFFSET, PICS},
    multitasking::TASKMANAGER,
    syscall::yield_now,
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

    let bytes = divisor.to_le_bytes();

    without_interrupts(|| {
        let prev_divisor = PIT_DIVISOR.swap(divisor, Ordering::SeqCst);

        // let mut cmd: Port<u8> = Port::new(0x43);
        let mut data: Port<u8> = Port::new(0x40);

        unsafe {
            // cmd.write((3 << 4) | 3);
            // Write first 8 bits
            data.write(bytes[0]);
            // Write upper 8 bits
            data.write(bytes[1]);
        }
    })
}

pub fn get_frequency() -> usize {
    return PIT_BASE_FREQUENCY / PIT_DIVISOR.load(Ordering::Relaxed) as usize;
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
    TIME_SINCE_BOOT.load(Ordering::Relaxed)
}

// pub fn sleep(ms: usize) {
//     let end_time = get_uptime() + ms;
//     while get_uptime() < end_time {
//         // Let next process have a go
//         yield_now();
//     }
// }

pub fn start_switching_tasks() {
    SWITCH_TASK.store(true, Ordering::Relaxed)
}

pub fn stop_switching_tasks() {
    SWITCH_TASK.store(false, Ordering::Relaxed)
}

wrap_function_registers!(tick => tick_handler);

extern "C" fn tick(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Get the amount of milliseconds per interrupt
    let freq = 1000 / get_frequency();
    // Increment the uptime counter
    TIME_SINCE_BOOT.fetch_add(freq, Ordering::Relaxed);

    // If timer is used for switching tasks switch task
    if SWITCH_TASK.load(Ordering::Relaxed) {
        TASKMANAGER
            .try_lock()
            .unwrap()
            .switch_task(stack_frame, regs);
    }
    unsafe { PICS.lock().notify_end_of_interrupt(PIC1_OFFSET) }
}
