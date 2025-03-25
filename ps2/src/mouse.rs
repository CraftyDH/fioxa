use input::mouse::MousePacket;
use kernel_userspace::input::InputServiceMessage;
use userspace::log::info;

use super::PS2Command;

// Keycodes
// https://www.win.tue.nl/~aeb/linux/kbd/scancodes-13.html

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum MouseTypeId {
    Standard,
    WithScrollWheel,
    WithExtraButtons,
}

#[derive(Debug)]
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
}

impl Mouse {
    pub fn new(command: PS2Command) -> Self {
        Self {
            command,
            mouse_type: MouseTypeId::Standard,
            packet_state: PS2MousePackets::None,
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
            info!("Res: {}", response);
            return Err("Mouse didn't acknolodge command");
        }
        Err("Mouse required too many command resends")
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
        info!("Mode: {}", mode);

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
            info!("Mode: {}", mode);
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
            info!("Mode: {}", mode);
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

    pub fn check_interrupts(&mut self) -> Option<InputServiceMessage> {
        let data: u8 = unsafe { self.command.data_port.read() };
        let mut res = None;
        self.packet_state = match (&self.packet_state, &self.mouse_type) {
            (PS2MousePackets::None, _) => PS2MousePackets::One(data),
            (PS2MousePackets::One(a), _) => PS2MousePackets::Two(*a, data),
            (PS2MousePackets::Two(a, b), MouseTypeId::Standard) => {
                res = Some(self.send_packet(*a, *b, data));
                PS2MousePackets::None
            }
            (_, MouseTypeId::Standard) => unreachable!(),
            (PS2MousePackets::Two(a, b), _) => PS2MousePackets::Three(*a, *b, data),
            (
                PS2MousePackets::Three(a, b, c),
                MouseTypeId::WithExtraButtons | MouseTypeId::WithScrollWheel,
            ) => {
                // Discard scroll wheel for now
                res = Some(self.send_packet(*a, *b, *c));
                PS2MousePackets::None
            }
        };
        res
    }

    pub fn send_packet(&mut self, p1: u8, p2: u8, p3: u8) -> InputServiceMessage {
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
        InputServiceMessage::MouseEvent(packet)
    }
}
