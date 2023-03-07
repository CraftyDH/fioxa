use alloc::sync::Arc;

use crossbeam_queue::ArrayQueue;
use kernel_userspace::stream::{StreamMessage, StreamMessageType};
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::{instructions::port::Port, structures::idt::InterruptStackFrame};

use crate::{
    interrupt_handler,
    ioapic::mask_entry,
    stream::{STREAM, STREAMS},
};

use super::{scancode::set2::ScancodeSet2, PS2Command};

static DECODER: Mutex<ScancodeSet2> = Mutex::new(ScancodeSet2::new());

lazy_static! {
    static ref KEYBOARD_QUEUE: STREAM = Arc::new(ArrayQueue::new(1000));
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
        let mut msg = StreamMessage {
            message_type: StreamMessageType::InlineData,
            timestamp: 0,
            data: Default::default(),
        };
        msg.write_data(key);

        if let Some(_) = KEYBOARD_QUEUE.force_push(msg) {
            println!("WARN: Keyboard buffer full dropping packets")
        }
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

        if let Some(_) = STREAMS
            .lock()
            .insert("input:keyboard", KEYBOARD_QUEUE.clone())
        {
            panic!("Stream already existed")
        }

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
