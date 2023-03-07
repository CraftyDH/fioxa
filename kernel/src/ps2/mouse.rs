use core::sync::atomic::{AtomicI8, AtomicU8, Ordering};

use alloc::sync::Arc;

use crossbeam_queue::ArrayQueue;
use kernel_userspace::stream::StreamMessage;
use lazy_static::lazy_static;
use x86_64::{instructions::port::Port, structures::idt::InterruptStackFrame};

use crate::{interrupt_handler, ioapic::mask_entry, stream::{STREAMS, STREAM}};

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
    static ref MOUSEPACKET_QUEUE: STREAM = Arc::new(ArrayQueue::new(1000));
}
static PACKETS_REQUIRED: AtomicI8 = AtomicI8::new(-1);
static POS: AtomicU8 = AtomicU8::new(0);

static PACKET_0: AtomicU8 = AtomicU8::new(0);
static PACKET_1: AtomicU8 = AtomicU8::new(0);
static PACKET_2: AtomicU8 = AtomicU8::new(0);

interrupt_handler!(interrupt_handler => mouse_int_handler);

pub fn interrupt_handler(_: InterruptStackFrame) {
    let mut port = Port::new(0x60);

    let data: u8 = unsafe { port.read() };

    let packets_required = PACKETS_REQUIRED.load(Ordering::SeqCst);
    let pos = POS.load(Ordering::SeqCst);

    // Packets not accepted
    if packets_required == -1 {
        return;
    }

    let reset = || {
        POS.store(0, Ordering::SeqCst);
    };

    match pos {
        0 => {
            if data & 0b00001000 == 0 {
                return;
            }
            PACKET_0.store(data, Ordering::SeqCst);
            POS.store(1, Ordering::SeqCst);
        }
        1 => {
            PACKET_1.store(data, Ordering::SeqCst);
            POS.store(2, Ordering::SeqCst);
        }
        2 => {
            if packets_required == 3 {
                let val0 = PACKET_0.load(Ordering::SeqCst);
                let val1 = PACKET_1.load(Ordering::SeqCst);
                send_packet(val0, val1, data);

                reset()
            } else {
                PACKET_2.store(data, Ordering::SeqCst);
                POS.store(3, Ordering::SeqCst);
            }
        }
        3 => {
            let val0 = PACKET_0.load(Ordering::SeqCst);
            let val1 = PACKET_1.load(Ordering::SeqCst);
            let val2 = PACKET_2.load(Ordering::SeqCst);
            send_packet(val0, val1, val2);
            reset()
        }
        // Shouln't be possible
        _ => unreachable!(),
    }
}

pub struct MousePacket {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub x_mov: i8,
    pub y_mov: i8,
}

pub fn send_packet(p1: u8, p2: u8, p3: u8) {
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

    let mut msg = StreamMessage {
        message_type: kernel_userspace::stream::StreamMessageType::InlineData,
        timestamp: 0,
        data: [0; 16],
    };

    msg.write_data(packet);

    if let Some(_) = MOUSEPACKET_QUEUE.force_push(msg) {
        println!("WARN: Mouse buffer full dropping packets")
    }
}

pub struct Mouse {
    command: PS2Command,
    mouse_type: MouseTypeId,
}

impl Mouse {
    pub const fn new(command: PS2Command) -> Self {
        Self {
            command,
            mouse_type: MouseTypeId::Standard,
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
            0 => {
                PACKETS_REQUIRED.store(3, Ordering::SeqCst);
                MouseTypeId::Standard
            }
            3 => {
                PACKETS_REQUIRED.store(4, Ordering::SeqCst);
                MouseTypeId::WithScrollWheel
            }
            4 => {
                PACKETS_REQUIRED.store(4, Ordering::SeqCst);
                MouseTypeId::WithExtraButtons
            }
            // Who knows, just emulate a standard
            _ => {
                PACKETS_REQUIRED.store(3, Ordering::SeqCst);
                MouseTypeId::Standard
            }
        };

        POS.store(0, Ordering::SeqCst);

        // Enable packet streaming (aka interrupts)
        self.send_command(0xF4)?;

        if let Some(_) = STREAMS
            .lock()
            .insert("input:mouse", MOUSEPACKET_QUEUE.clone())
        {
            panic!("Stream already existed")
        }

        Ok(())
    }
    pub fn receive_interrupts(&self) {
        mask_entry(12, true);
    }
}
