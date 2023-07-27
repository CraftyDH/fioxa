use alloc::{sync::Arc, vec::Vec};
use kernel_userspace::{
    service::get_public_service_id,
    syscall::{receive_service_message_blocking, service_subscribe, spawn_thread},
};
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

use crate::log;

use self::{keyboard::Keyboard, mouse::Mouse};

pub mod keyboard;
pub mod mouse;
pub mod scancode;
pub mod translate;

pub fn main() {
    log!("Initalizing PS2 devices...");
    let mut ps2_controller = PS2Controller::new();

    if let Err(e) = ps2_controller.initialize() {
        log!("PS2 Controller failed to init because: {}", e);
    }

    let mut buffer = Vec::new();

    let kb_event = get_public_service_id("INTERRUPTS:KB", &mut buffer).unwrap();
    let mouse_event = get_public_service_id("INTERRUPTS:MOUSE", &mut buffer).unwrap();

    service_subscribe(kb_event);
    service_subscribe(mouse_event);

    // TODO: Once some form of multi wait is implemented use 1 thread.

    let controller = Arc::new(Mutex::new(ps2_controller));

    let c = controller.clone();
    spawn_thread(move || loop {
        let mut buffer = Vec::new();
        loop {
            let message = receive_service_message_blocking(mouse_event, &mut buffer).unwrap();
            match message.message {
                kernel_userspace::service::ServiceMessageType::InterruptEvent => {
                    c.lock().mouse.check_interrupts()
                }
                _ => unimplemented!(),
            }
        }
    });

    loop {
        let message = receive_service_message_blocking(kb_event, &mut buffer).unwrap();
        match message.message {
            kernel_userspace::service::ServiceMessageType::InterruptEvent => {
                controller.lock().keyboard.check_interrupts()
            }
            _ => unimplemented!(),
        }
    }
}
pub struct PS2Command {
    data_port: Port<u8>,
    status_port: PortReadOnly<u8>,
    command_port: PortWriteOnly<u8>,
}

impl PS2Command {
    pub const fn new() -> Self {
        Self {
            data_port: Port::new(0x60),
            status_port: PortReadOnly::new(0x64),
            command_port: PortWriteOnly::new(0x64),
        }
    }

    fn read(&mut self) -> Result<u8, &'static str> {
        let timeout = 100_000;
        for _ in 0..timeout {
            let test = unsafe { self.status_port.read() };
            if test & 0b1 == 0b1 {
                return Ok(unsafe { self.data_port.read() });
            }
        }
        Err("PS2 read timeout")
    }

    fn wait_write(&mut self) -> Result<(), &'static str> {
        let timeout = 100_000;
        for _ in 0..timeout {
            let test = unsafe { self.status_port.read() };
            if test & 0b10 == 0 {
                return Ok(());
            }
        }
        Err("PS2 write timeout")
    }

    fn write_command(&mut self, command: u8) -> Result<(), &'static str> {
        self.wait_write()?;
        unsafe { self.command_port.write(command) };
        Ok(())
    }

    fn write_data(&mut self, data: u8) -> Result<(), &'static str> {
        self.wait_write()?;
        unsafe { self.data_port.write(data) };
        Ok(())
    }
}

pub struct PS2Controller {
    command: PS2Command,
    keyboard: Keyboard,
    mouse: Mouse,
}

impl PS2Controller {
    pub fn new() -> Self {
        // Values from https://wiki.osdev.org/%228042%22_PS/2_Controller
        let command = PS2Command::new();
        let keyboard = Keyboard::new(PS2Command::new());
        let mouse = Mouse::new(PS2Command::new());

        Self {
            command,
            keyboard,
            mouse,
        }
    }

    pub fn initialize(&mut self) -> Result<(), &'static str> {
        // Disable both devices
        // Disable port 1
        self.command.write_command(0xAD)?;
        // Disable port 2
        self.command.write_command(0xA7)?;

        // Flush output buffer
        let timeout = 100_000;
        for _ in 0..timeout {
            let test = unsafe { self.command.status_port.read() };
            if test & 0b1 == 0b1 {
                unsafe { self.command.data_port.read() };
            }
        }
        // Set controller bytes
        self.command.write_command(0x20)?;
        let mut configuration = self.command.read()?;
        println!("PS2 controller config, {:b}", configuration);
        // Clear bits 0, 1, 6
        configuration &= !(1 | 0b10 | 1 << 6);

        // Write config back
        self.command.write_command(0x60)?;
        self.command.write_data(configuration)?;
        // TODO: Check if only one lane is available

        // Perform self test
        self.command.write_command(0xAA)?;
        let result = self.command.read()?;
        if result != 0x55 {
            return Err("PS2 Controller failed self test");
        }

        // Test keyboard port
        let keyboard = self.command.write_command(0xAB).and_then(|_| {
            // 0 indicates a successful test
            if self.command.read()? != 0 {
                return Err("Keyboard port failed test");
            }
            Ok(())
        });

        // Test mouse port
        let mouse = self.command.write_command(0xA9).and_then(|_| {
            // 0 indicates a successful test
            if self.command.read()? != 0 {
                return Err("Mouse port failed test");
            }
            Ok(())
        });

        // Initialize keyboard if test passed
        let keyboard = keyboard.and_then(|_| self.keyboard.initialize());

        // Initialize mouse if test passed
        let mouse = mouse.and_then(|_| self.mouse.initialize());

        // Now enable interrupts
        let keyboard = keyboard.map(|_| self.keyboard.receive_interrupts());
        let mouse = mouse.map(|_| self.mouse.receive_interrupts());

        // If keyboard failed to initalize print the error reason
        if let Err(e) = keyboard {
            println!("Keyboard failed to init because: {}", e)
        }

        // If mouse failed to initalize print the error reason
        if let Err(e) = mouse {
            println!("Mouse failed to init becuase: {}", e)
        };

        // Even if there was an error with the keyboard or mouse we can still continue
        // And use the working one
        Ok(())
    }
}
