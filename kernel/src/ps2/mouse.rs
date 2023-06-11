use core::sync::atomic::AtomicBool;

use crossbeam_queue::ArrayQueue;
use input::mouse::MousePacket;
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    input::InputServiceMessage,
    service::{
        generate_tracking_number, SendServiceMessageDest, ServiceMessage, ServiceMessageType,
    },
    syscall::{get_pid, send_service_message, service_create},
};
use lazy_static::lazy_static;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{interrupt_handler, ioapic::mask_entry, service::PUBLIC_SERVICES};

use super::PS2Command;

// Keycodes
// https://www.win.tue.nl/~aeb/linux/kbd/scancodes-13.html

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum MouseTypeId {
    Standard,
    WithScrollWheel,
    WithExtraButtons,
}

lazy_static! {
    static ref MOUSEPACKET_QUEUE: ArrayQueue<MousePacket> = ArrayQueue::new(250);
}

interrupt_handler!(interrupt_handler => mouse_int_handler);

static INT_WAITING: AtomicBool = AtomicBool::new(false);

pub fn interrupt_handler(_: InterruptStackFrame) {
    INT_WAITING.store(true, core::sync::atomic::Ordering::SeqCst)
}

enum PS2MousePackets {
    None,
    One(u8),
    Two(u8, u8),
    Three(u8, u8, u8),
}

pub struct Mouse {
    command: PS2Command,
    mouse_type: MouseTypeId,
    packet_state: PS2MousePackets,
    mouse_service: ServiceID,
    current_pid: ProcessID,
}

impl Mouse {
    pub fn new(command: PS2Command) -> Self {
        let mouse_service = service_create();
        PUBLIC_SERVICES.lock().insert("INPUT:MOUSE", mouse_service);

        Self {
            command,
            mouse_type: MouseTypeId::Standard,
            packet_state: PS2MousePackets::None,
            mouse_service,
            current_pid: get_pid(),
        }
    }

    fn send_command(&mut self, command: u8) -> Result<(), &'static str> {
        for _ in 0..10 {
            // Say we are talking to the mouse
            self.command.write_command(0xD4)?;
            // Write the command
            self.command.write_data(command)?;
            // Check for ACK
            let response = self.command.read()?;
            // If a resend packet is encounted
            if response == 0xFE {
                continue;
            }
            // 0xFA is successcode,
            // TODO: it sends 0 somethimes; why?
            else if response == 0xFA {
                return Ok(());
            }
            println!("Res: {}", response);
            return Err("Mouse didn't acknolodge command");
        }
        return Err("Mouse required too many command resends");
    }

    pub fn initialize(&mut self) -> Result<(), &'static str> {
        // Enable
        self.command.write_command(0xA8)?;

        // Reset
        self.send_command(0xFF)?;

        // Mouse will respond 0xAA then 0 on reset
        // Ensure sucessful reset by testing for pass of 0xAA
        if self.command.read()? != 0xAA {
            return Err("Mouse failed self test");
        }

        // Ensure sucessful reset by testing for pass of 0
        if self.command.read()? != 0 {
            return Err("Mouse failed self test");
        }

        // Enable device interrupts
        self.command.write_command(0x20)?;
        let configuration = self.command.read()?;
        self.command.write_command(0x60)?;
        self.command.write_data(configuration | 0b10)?;

        // Default setting
        self.send_command(0xF6)?;

        // Find current id
        self.send_command(0xF2)?;

        let mut mode = self.command.read()?;
        println!("Mode: {}", mode);

        if mode == 0 {
            // Try and upgrade
            self.send_command(0xF3)?;
            self.send_command(200)?;

            self.send_command(0xF3)?;
            self.send_command(100)?;

            self.send_command(0xF3)?;
            self.send_command(80)?;

            self.send_command(0xF2)?;
            mode = self.command.read()?;
            println!("Mode: {}", mode);
        }
        if mode == 3 {
            // Try and upgrade again
            self.send_command(0xF3)?;
            self.send_command(200)?;

            self.send_command(0xF3)?;
            self.send_command(100)?;

            self.send_command(0xF3)?;
            self.send_command(80)?;

            self.send_command(0xF2)?;
            let mode = self.command.read()?;
            println!("Mode: {}", mode);
        }

        // Save mouse type
        self.mouse_type = match mode {
            0 => MouseTypeId::Standard,
            3 => MouseTypeId::WithScrollWheel,
            4 => MouseTypeId::WithExtraButtons,
            // Who knows, just emulate a standard
            _ => MouseTypeId::Standard,
        };

        // Enable packet streaming (aka interrupts)
        self.send_command(0xF4)?;

        Ok(())
    }

    pub fn receive_interrupts(&self) {
        mask_entry(12, true);
    }

    pub fn check_interrupts(&mut self) {
        loop {
            let waiting = INT_WAITING.swap(false, core::sync::atomic::Ordering::SeqCst);

            if !waiting {
                return;
            }
            let data: u8 = unsafe { self.command.data_port.read() };

            self.packet_state = match (&self.packet_state, &self.mouse_type) {
                (PS2MousePackets::None, _) => PS2MousePackets::One(data),
                (PS2MousePackets::One(a), _) => PS2MousePackets::Two(*a, data),
                (PS2MousePackets::Two(a, b), MouseTypeId::Standard) => {
                    self.send_packet(*a, *b, data);
                    PS2MousePackets::None
                }
                (_, MouseTypeId::Standard) => unreachable!(),
                (PS2MousePackets::Two(a, b), _) => PS2MousePackets::Three(*a, *b, data),
                (
                    PS2MousePackets::Three(a, b, c),
                    MouseTypeId::WithExtraButtons | MouseTypeId::WithScrollWheel,
                ) => {
                    // Discard scroll wheel for now
                    self.send_packet(*a, *b, *c);
                    PS2MousePackets::None
                }
            };
        }
    }

    pub fn send_packet(&mut self, p1: u8, p2: u8, p3: u8) {
        let left = p1 & 0b0000_0001 > 0;
        let right = p1 & 0b0000_0010 > 0;
        let middle = p1 & 0b0000_0100 > 0;

        let mut x: i16 = p2.into();
        // X is negative
        if p1 & 0b0001_0000 > 0 {
            x = -(256 - x)
        }

        let mut y: i16 = p3.into();
        // X is negative
        if p1 & 0b0010_0000 > 0 {
            y = 256 - y;
        } else {
            y = -y;
        }

        let packet = MousePacket {
            left,
            right,
            middle,
            x_mov: x as i8,
            y_mov: y as i8,
        };

        send_service_message(&ServiceMessage {
            service_id: self.mouse_service,
            sender_pid: self.current_pid,
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToSubscribers,
            message: ServiceMessageType::Input(InputServiceMessage::MouseEvent(packet)),
        })
        .unwrap()
    }
}
