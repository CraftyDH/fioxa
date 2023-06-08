use crossbeam_queue::ArrayQueue;
use input::keyboard::KeyboardEvent;
use kernel_userspace::{
    service::{send_service_message, SID},
    syscall::service_create,
};
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::{instructions::port::Port, structures::idt::InterruptStackFrame};

use crate::{interrupt_handler, ioapic::mask_entry, service::PUBLIC_SERVICES};

use super::{scancode::set2::ScancodeSet2, PS2Command};

static DECODER: Mutex<ScancodeSet2> = Mutex::new(ScancodeSet2::new());

lazy_static! {
    static ref KEYBOARD_QUEUE: ArrayQueue<KeyboardEvent> = ArrayQueue::new(100);
    static ref KEYBOARD_SERVICE: SID = {
        let sid = service_create();
        PUBLIC_SERVICES.lock().insert("INPUT:KB", sid);
        sid
    };
}

pub struct Keyboard {
    command: PS2Command,
}

interrupt_handler!(interrupt_handler => keyboard_int_handler);

pub fn interrupt_handler(_: InterruptStackFrame) {
    let mut port = Port::new(0x60);

    let scancode: u8 = unsafe { port.read() };

    let res = DECODER.lock().add_byte(scancode);
    if let Some(key) = res {
        if let Some(_) = KEYBOARD_QUEUE.force_push(key) {
            println!("WARN: Keyboard buffer full dropping packets")
        }
    }
}

pub fn dispatch_events() {
    while let Some(msg) = KEYBOARD_QUEUE.pop() {
        send_service_message(
            *KEYBOARD_SERVICE,
            kernel_userspace::service::MessageType::Announcement,
            0,
            0,
            msg,
            0,
        )
    }
}

impl Keyboard {
    pub const fn new(command: PS2Command) -> Self {
        Self { command }
    }

    fn send_command(&mut self, command: u8) -> Result<(), &'static str> {
        for _ in 0..10 {
            // Write the command
            self.command.write_data(command)?;
            // Check for ACK
            let response = self.command.read()?;
            // If a resend packet is encounted
            if response == 0xFE {
                continue;
            // 0xFA is successcode
            } else if response != 0xFA {
                return Err("Keyboard didn't acknolodge command");
            }
            return Ok(());
        }
        return Err("Keyboard required too many command resends");
    }

    pub fn initialize(&mut self) -> Result<(), &'static str> {
        // Enable kb interrupts
        self.command.write_command(0xAE)?;

        // Reset
        self.send_command(0xFF)?;
        // Ensure sucessful reset by testing for pass of 0xAA
        if self.command.read()? != 0xAA {
            return Err("Keyboard failed self test");
        }

        // Enable device interrupts
        self.command.write_command(0x20)?;
        let configuration = self.command.read()?;
        self.command.write_command(0x60)?;
        self.command.write_data(configuration | 0b1)?;

        // Set keyboard layout to scancode set 2
        self.send_command(0xF0)?;
        self.send_command(2)?;

        // Init the service
        core::hint::black_box(*KEYBOARD_SERVICE);

        Ok(())
    }

    pub fn receive_interrupts(&self) {
        mask_entry(1, true);
    }

    // fn update_leds(&mut self) {
    //     // Create state packet as stated here
    //     // https://wiki.osdev.org/PS/2_Keyboard
    //     let state =
    //         (self.caps_lock as u8) << 2 | (self.num_lock as u8) << 1 | self.scroll_lock as u8;
    //     // 0xED is set LEDS
    //     if let Err(e) = self
    //         .send_command(0xED)
    //         .and_then(|_| self.send_command(state))
    //     {
    //         println!("WARN: Kb failed to update leds: {}", e)
    //     }
    // }
}
