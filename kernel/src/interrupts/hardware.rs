use pic8259::ChainedPics;
use spin::{self, Mutex};
use x86_64::{
    instructions::interrupts::without_interrupts,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

pub const PIC1_OFFSET: u8 = 0x20;
pub const PIC2_OFFSET: u8 = PIC1_OFFSET + 8;

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

// #[derive(Clone, Copy)]
// pub enum HandlerFns {
//     Fn(fn()),
//     Wrapped(extern "x86-interrupt" fn(InterruptStackFrame)),
// }

pub static HANDLERS: Mutex<[Option<fn(InterruptStackFrame)>; 16]> = Mutex::new([None; 16]);

pub fn set_handler_and_enable_irq(irq: u8, handler: fn(InterruptStackFrame)) {
    without_interrupts(|| {
        HANDLERS.lock()[irq as usize] = Some(handler);

        // Enable the irq
        let mut pics = PICS.lock();
        let mut mask = unsafe { pics.read_masks() };

        // Example
        // Keyboard = 1
        // 0b0000_0001
        // Bit shift by 1
        // 0b0000_0010
        // Invert
        // 0b1111_1101
        // Now and with mask to enable

        if irq > 8 {
            mask[1] &= !(1 << (irq - 8));
        } else {
            mask[0] &= !(1 << irq)
        }
        unsafe { pics.write_masks(mask[0], mask[1]) };
    })
}

#[allow(dead_code)]
pub fn remove_handler_and_clear_irq(irq: u8) {
    without_interrupts(|| {
        HANDLERS.lock()[irq as usize] = None;

        // Enable the irq
        let mut pics = PICS.lock();
        let mut mask = unsafe { pics.read_masks() };

        // Example
        // Keyboard = 1
        // 0b0000_0001
        // Bit shift by 1
        // 0b0000_0010
        // Now or with mask to disable

        if irq > 8 {
            mask[1] |= 1 << irq - 8;
        } else {
            mask[0] |= 1 << irq
        }
        unsafe { pics.write_masks(mask[0], mask[1]) };
    })
}

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC1_OFFSET, PIC2_OFFSET) });

pub fn set_hardware_idt(idt: &mut InterruptDescriptorTable) {
    // Set handlers
    idt[PIC1_OFFSET as usize + 0].set_handler_fn(crate::pit::tick_handler);
    idt[PIC1_OFFSET as usize + 1].set_handler_fn(irq1_handler);
    idt[PIC1_OFFSET as usize + 2].set_handler_fn(irq2_handler);
    idt[PIC1_OFFSET as usize + 3].set_handler_fn(irq3_handler);
    idt[PIC1_OFFSET as usize + 4].set_handler_fn(irq4_handler);
    idt[PIC1_OFFSET as usize + 5].set_handler_fn(irq5_handler);
    idt[PIC1_OFFSET as usize + 6].set_handler_fn(irq6_handler);
    idt[PIC1_OFFSET as usize + 7].set_handler_fn(irq7_handler);
    idt[PIC1_OFFSET as usize + 8].set_handler_fn(irq8_handler);
    idt[PIC1_OFFSET as usize + 9].set_handler_fn(irq9_handler);
    idt[PIC1_OFFSET as usize + 10].set_handler_fn(irq10_handler);
    idt[PIC1_OFFSET as usize + 11].set_handler_fn(irq11_handler);
    idt[PIC1_OFFSET as usize + 12].set_handler_fn(irq12_handler);
    idt[PIC1_OFFSET as usize + 13].set_handler_fn(irq13_handler);
    idt[PIC1_OFFSET as usize + 14].set_handler_fn(irq14_handler);
    idt[PIC1_OFFSET as usize + 15].set_handler_fn(irq15_handler);
}

/// Generates a handler for each PIC lane.
/// Calls the appropiate handler in the HANDLERS list
#[macro_export]
macro_rules! interrupt_handler {
    ($handler: ident, $irq:expr) => {
        pub extern "x86-interrupt" fn $handler(stack_frame: InterruptStackFrame) {
            // Find the relevent handler and call it
            match &HANDLERS.lock()[$irq as usize] {
                Some(func) => func(stack_frame),
                None => println!(
                    "WARNING: Interrupt number {} received from the PIC...",
                    $irq
                ),
            };

            // Notify end of interrupt
            unsafe { PICS.lock().notify_end_of_interrupt(PIC1_OFFSET + $irq) }
        }
    };
}

interrupt_handler!(irq1_handler, 1);
interrupt_handler!(irq2_handler, 2);
interrupt_handler!(irq3_handler, 3);
interrupt_handler!(irq4_handler, 4);
interrupt_handler!(irq5_handler, 5);
interrupt_handler!(irq6_handler, 6);
interrupt_handler!(irq7_handler, 7);
interrupt_handler!(irq8_handler, 8);
interrupt_handler!(irq9_handler, 9);
interrupt_handler!(irq10_handler, 10);
interrupt_handler!(irq11_handler, 11);
interrupt_handler!(irq12_handler, 12);
interrupt_handler!(irq13_handler, 13);
interrupt_handler!(irq14_handler, 14);
interrupt_handler!(irq15_handler, 15);
