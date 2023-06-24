use alloc::{boxed::Box, vec::Vec};
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    input::InputServiceMessage,
    service::{
        generate_tracking_number, SendServiceMessageDest, ServiceMessage, ServiceMessageType,
    },
    syscall::{get_pid, send_service_message, service_create},
};

use crate::{ioapic::mask_entry, service::PUBLIC_SERVICES};

use super::{
    scancode::{set2::ScancodeSet2, Scancode},
    PS2Command,
};

pub struct Keyboard {
    command: PS2Command,
    keyboard_service: ServiceID,
    decoder: Box<dyn Scancode>,
    current_pid: ProcessID,
    send_buffer: Vec<u8>,
}

impl Keyboard {
    pub fn new(command: PS2Command) -> Self {
        let keyboard_service = service_create();
        PUBLIC_SERVICES.lock().insert("INPUT:KB", keyboard_service);

        Self {
            command,
            keyboard_service,
            decoder: Box::new(ScancodeSet2::new()),
            current_pid: get_pid(),
            send_buffer: Default::default(),
        }
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

        Ok(())
    }

    pub fn receive_interrupts(&self) {
        mask_entry(1, true);
    }

    pub fn check_interrupts(&mut self) {
        let scancode: u8 = unsafe { self.command.data_port.read() };

        let res = self.decoder.add_byte(scancode);
        if let Some(key) = res {
            send_service_message(
                &ServiceMessage {
                    service_id: self.keyboard_service,
                    sender_pid: self.current_pid,
                    tracking_number: generate_tracking_number(),
                    destination: SendServiceMessageDest::ToSubscribers,
                    message: ServiceMessageType::Input(InputServiceMessage::KeyboardEvent(key)),
                },
                &mut self.send_buffer,
            )
            .unwrap()
        }
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
