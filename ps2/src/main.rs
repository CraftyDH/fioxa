#![no_std]
#![no_main]

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use alloc::vec::Vec;
use kernel_userspace::{
    backoff_sleep,
    channel::{channel_create_rs, channel_read_rs, channel_write_val, ChannelReadResult},
    interrupt::{interrupt_acknowledge, interrupt_set_port},
    object::{object_wait_port_rs, KernelReference, ObjectSignal},
    port::{port_create, port_wait_rs},
    process::{get_handle, publish_handle},
    syscall::exit,
    INT_KB, INT_MOUSE,
};
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

use self::{keyboard::Keyboard, mouse::Mouse};

pub mod keyboard;
pub mod mouse;
pub mod scancode;
pub mod translate;

#[export_name = "_start"]
pub extern "C" fn main() {
    println!("Initalizing PS2 devices...");
    let mut ps2_controller = PS2Controller::new();

    if let Err(e) = ps2_controller.initialize() {
        panic!("PS2 Controller failed to init because: {}", e);
    }

    let mut buffer = Vec::with_capacity(100);
    let mut handles_buffer = Vec::with_capacity(1);

    let interrupts = backoff_sleep(|| get_handle("INTERRUPTS"));

    channel_write_val(interrupts, &INT_KB, &[]);
    match channel_read_rs(interrupts, &mut buffer, &mut handles_buffer) {
        ChannelReadResult::Ok => (),
        _ => panic!(),
    }
    let kb_ev = handles_buffer[0];

    channel_write_val(interrupts, &INT_MOUSE, &[]);
    match channel_read_rs(interrupts, &mut buffer, &mut handles_buffer) {
        ChannelReadResult::Ok => (),
        _ => panic!(),
    }
    let mouse_ev = handles_buffer[0];

    let kb_cbk = 1;
    let ms_cbk = 2;
    let kb_srv_cbk = 3;
    let ms_srv_cbk = 4;

    let (kb_service, kb_right) = channel_create_rs();
    publish_handle("INPUT:KB", kb_right.id());

    let (ms_service, m_right) = channel_create_rs();
    publish_handle("INPUT:MOUSE", m_right.id());

    let port = port_create();

    interrupt_set_port(kb_ev, port, kb_cbk);
    interrupt_set_port(mouse_ev, port, ms_cbk);

    ps2_controller.flush();

    println!("PS2 Ready");

    object_wait_port_rs(kb_service.id(), port, ObjectSignal::READABLE, kb_srv_cbk);
    object_wait_port_rs(ms_service.id(), port, ObjectSignal::READABLE, ms_srv_cbk);

    let mut kb_listeners: Vec<KernelReference> = Vec::new();
    let mut ms_listeners: Vec<KernelReference> = Vec::new();

    loop {
        let ev = port_wait_rs(port);

        if ev.key == kb_cbk {
            if let Some(ev) = ps2_controller.keyboard.check_interrupts() {
                let message = kernel_userspace::input::InputServiceMessage::KeyboardEvent(ev);
                kb_listeners.retain(|l| channel_write_val(l.id(), &message, &[]));
            }
            interrupt_acknowledge(kb_ev);
        } else if ev.key == ms_cbk {
            if let Some(message) = ps2_controller.mouse.check_interrupts() {
                ms_listeners.retain(|l| channel_write_val(l.id(), &message, &[]));
            }
            interrupt_acknowledge(mouse_ev);
        } else if ev.key == kb_srv_cbk {
            match channel_read_rs(kb_service.id(), &mut buffer, &mut handles_buffer) {
                ChannelReadResult::Ok => (),
                e => panic!("{e:?}"),
            }
            kb_listeners.push(KernelReference::from_id(handles_buffer[0]));
        } else if ev.key == ms_srv_cbk {
            match channel_read_rs(ms_service.id(), &mut buffer, &mut handles_buffer) {
                ChannelReadResult::Ok => (),
                e => panic!("{e:?}"),
            }
            ms_listeners.push(KernelReference::from_id(handles_buffer[0]));
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

    pub fn flush(&mut self) {
        let timeout = 100_000;
        for _ in 0..timeout {
            let test = unsafe { self.command.status_port.read() };
            if test & 0b1 == 0b1 {
                unsafe { self.command.data_port.read() };
            }
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

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
