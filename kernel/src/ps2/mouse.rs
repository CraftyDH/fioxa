use core::sync::atomic::{AtomicI8, AtomicU8, Ordering};

use conquer_once::spin::OnceCell;
use crossbeam_queue::ArrayQueue;
use x86_64::{
    instructions::{interrupts::without_interrupts, port::Port},
    structures::idt::InterruptStackFrame,
};

use crate::{
    interrupt_handler,
    screen::gop::{Pos, WRITER},
};

use super::PS2Command;

const MOUSE_POINTER: &[u16; 16] = &[
    0b1111111111000000,
    0b1111111110000000,
    0b1111111100000000,
    0b1111111000000000,
    0b1111110000000000,
    0b1111100000000000,
    0b1111000000000000,
    0b1110000000000000,
    0b1100000000000000,
    0b1000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
];

// Keycodes
// https://www.win.tue.nl/~aeb/linux/kbd/scancodes-13.html

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum MouseTypeId {
    Standard,
    WithScrollWheel,
    WithExtraButtons,
}

static MOUSEPACKET_QUEUE: OnceCell<ArrayQueue<MousePacketState>> = OnceCell::uninit();
static PACKETS_REQUIRED: AtomicI8 = AtomicI8::new(-1);
static POS: AtomicU8 = AtomicU8::new(0);

static PACKET_0: AtomicU8 = AtomicU8::new(0);
static PACKET_1: AtomicU8 = AtomicU8::new(0);
static PACKET_2: AtomicU8 = AtomicU8::new(0);

#[derive(Debug)]
enum MousePacketState {
    ThreePackets(u8, u8, u8),
    FourPackets(u8, u8, u8, u8),
}

interrupt_handler!(interrupt_handler => mouse_int_handler);

pub fn interrupt_handler(_: InterruptStackFrame) {
    if let Ok(queue) = MOUSEPACKET_QUEUE.try_get() {
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
                    if let Some(_) =
                        queue.force_push(MousePacketState::ThreePackets(val0, val1, data))
                    {
                        println!("WARN: Mouse buffer full dropping packets")
                    }
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
                if let Some(_) =
                    queue.force_push(MousePacketState::FourPackets(val0, val1, val2, data))
                {
                    println!("WARN: Mouse buffer full dropping packets")
                }
                reset()
            }
            // Shouln't be possible
            _ => unreachable!(),
        }
    }
}

pub struct Mouse {
    command: PS2Command,
    mouse_type: MouseTypeId,
    pos: Pos,
}

impl Mouse {
    pub const fn new(command: PS2Command) -> Self {
        Self {
            command,
            mouse_type: MouseTypeId::Standard,
            pos: Pos { x: 0, y: 0 },
        }
    }

    pub fn check_packets(&mut self) {
        if let Ok(packet_queue) = MOUSEPACKET_QUEUE.try_get() {
            while let Some(packet) = packet_queue.pop() {
                self.handle_packet(packet)
            }
        }
    }

    fn handle_packet(&mut self, packet: MousePacketState) {
        // Handle first bits
        if self.mouse_type == MouseTypeId::Standard {
            let (p1, p2, p3) = {
                if let MousePacketState::ThreePackets(p1, p2, p3) = packet {
                    (p1, p2, p3)
                } else {
                    println!("Received a 4 packet mouse packet when not expecting");
                    return;
                }
            };
            self.handle_first_3_packets(p1, p2, p3);
        }
        if self.mouse_type == MouseTypeId::WithScrollWheel
            || self.mouse_type == MouseTypeId::WithExtraButtons
        {
            let (p1, p2, p3, _p4) = {
                if let MousePacketState::FourPackets(p1, p2, p3, p4) = packet {
                    (p1, p2, p3, p4)
                } else {
                    println!("Received a 3 packet mouse packet when not expecting");
                    return;
                }
            };
            self.handle_first_3_packets(p1, p2, p3);
        }
    }

    fn handle_first_3_packets(&mut self, p1: u8, p2: u8, p3: u8) {
        let mut _middle = false;
        let mut _right = false;
        let mut _left = false;

        // Check for overflow for Both Y and X
        // Probs a problem
        if p1 & 0b1100_0000 > 0 {
            // We just ignore the packet
            return;
        }

        let mut colour: u32 = 0x50_50_50;

        if p1 & 0b0000_0001 > 0 {
            _left = true;
            colour |= 0xFF_00_00;
        }

        if p1 & 0b0000_0010 > 0 {
            _right = true;
            colour |= 0x00_00_FF;
        }

        if p1 & 0b0000_0100 > 0 {
            _middle = true;
            colour |= 0x00_FF_00;
        }

        // X is negative
        if (p1 & 0b0001_0000) == 0b0001_0000 {
            // let p2_new = 256 - p2 as usize;
            // self.pos.x.checked_sub(p2_new).unwrap_or(0);
            self.pos.x = self.pos.x.checked_sub(256 - p2 as usize).unwrap_or(0);
        } else {
            self.pos.x += p2 as usize;
        }

        // Y is negative
        // It is invertied by default because why not
        // println!("Y: {} Yi: {}", p3, 255 - p3 as usize);
        // let p3 = p3 / 25;
        if (p1 & 0b0010_0000) == 0b0010_0000 {
            self.pos.y += 256 - p3 as usize;
        } else {
            // if p3 != 0 {
            self.pos.y = self.pos.y.checked_sub(p3 as usize).unwrap_or(0);
            // }
        }
        // println!("x: {} y: {}", self.pos.x, self.pos.y);
        without_interrupts(|| {
            let gop_mutex = &mut WRITER.lock();
            let gop_info = &gop_mutex.gop;

            if self.pos.x > gop_info.horizonal - 8 {
                self.pos.x = gop_info.horizonal - 8
            }

            if self.pos.y > gop_info.vertical - 16 {
                self.pos.y = gop_info.vertical - 16
            }
            // let mapper = unsafe { get_uefi_active_mapper() };
            // if (unsafe { pml4_ptr } != 0) {
            //     unsafe { load_into_cr3(pml4_ptr) };
            // }
            gop_mutex.draw_cursor(self.pos, colour, MOUSE_POINTER);
            // mapper.load_into_cr3();
        });
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

        Ok(())
    }
    pub fn receive_interrupts(&self) {
        MOUSEPACKET_QUEUE
            .try_init_once(|| ArrayQueue::new(5000))
            .unwrap();
    }
}
