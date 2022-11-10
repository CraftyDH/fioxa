use conquer_once::spin::OnceCell;
use crossbeam_queue::ArrayQueue;
use spin::Mutex;
use x86_64::{instructions::port::Port, structures::idt::InterruptStackFrame};

use crate::interrupt_handler;

use super::{
    scancode::{
        keys::{RawKeyCode, RawKeyCodeState},
        set2::ScancodeSet2,
    },
    translate::{translate_raw_keycode, KeyCode},
    PS2Command,
};

static DECODER: Mutex<ScancodeSet2> = Mutex::new(ScancodeSet2::new());
static SCANCODE_QUEUE: OnceCell<ArrayQueue<RawKeyCodeState>> = OnceCell::uninit();

pub struct Keyboard {
    command: PS2Command,
    caps_lock: bool,
    num_lock: bool,
    scroll_lock: bool,
    lshift: bool,
    rshift: bool,
}

interrupt_handler!(interrupt_handler => keyboard_int_handler);

pub fn interrupt_handler(_: InterruptStackFrame) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        let mut port = Port::new(0x60);

        let scancode: u8 = unsafe { port.read() };

        let res = DECODER.lock().add_byte(scancode);
        if let Some(key) = res {
            if let Some(_) = queue.force_push(key) {
                println!("WARN: Keyboard buffer full dropping packets")
            }
        }
    }
}

impl Keyboard {
    pub const fn new(command: PS2Command) -> Self {
        Self {
            command,
            caps_lock: false,
            num_lock: false,
            scroll_lock: false,
            lshift: false,
            rshift: false,
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

    pub fn check_packets(&mut self) {
        if let Ok(queue) = SCANCODE_QUEUE.try_get() {
            while let Some(scan_code) = queue.pop() {
                self.handle_code(scan_code)
            }
        }
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
        SCANCODE_QUEUE
            .try_init_once(|| ArrayQueue::new(1000))
            .unwrap();
    }

    fn update_leds(&mut self) {
        // Create state packet as stated here
        // https://wiki.osdev.org/PS/2_Keyboard
        let state =
            (self.caps_lock as u8) << 2 | (self.num_lock as u8) << 1 | self.scroll_lock as u8;
        // 0xED is set LEDS
        if let Err(e) = self
            .send_command(0xED)
            .and_then(|_| self.send_command(state))
        {
            println!("WARN: Kb failed to update leds: {}", e)
        }
    }

    fn handle_code(&mut self, scan_code: RawKeyCodeState) {
        match scan_code {
            RawKeyCodeState::Up(code) => match code {
                RawKeyCode::LeftShift => self.lshift = false,
                RawKeyCode::RightShift => self.rshift = false,
                // RawKeyCode::CapsLock => self.caps_lock = false,
                // RawKeyCode::NumLock => self.num_lock = false,
                _ => {}
            },
            RawKeyCodeState::Down(code) => match code {
                RawKeyCode::LeftShift => self.lshift = true,
                RawKeyCode::RightShift => self.rshift = true,
                RawKeyCode::CapsLock => {
                    self.caps_lock = !self.caps_lock;
                    self.update_leds()
                }
                RawKeyCode::NumLock => {
                    self.num_lock = !self.num_lock;
                    self.update_leds()
                }
                RawKeyCode::ScrollLock => {
                    self.scroll_lock = !self.scroll_lock;
                    self.update_leds()
                }
                _ => {
                    let shift = self.lshift | self.rshift;
                    match translate_raw_keycode(code, shift, self.caps_lock, self.num_lock) {
                        KeyCode::Unicode(key) => print!("{}", key),
                        KeyCode::SpecialCodes(key) => print!("{}", key),
                    }
                }
            },
        }
    }
}
