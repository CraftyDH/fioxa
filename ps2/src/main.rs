#![no_std]
#![no_main]

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use alloc::vec::Vec;
use kernel_sys::{
    syscall::sys_exit,
    types::{ObjectSignal, SyscallResult},
};
use kernel_userspace::{
    backoff_sleep, channel::Channel, interrupt::Interrupt, port::Port, process::get_handle, INT_KB,
    INT_MOUSE,
};
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

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

    let interrupts = Channel::from_handle(backoff_sleep(|| get_handle("INTERRUPTS")));

    let (_, mut kb) = interrupts.call_val::<1, _, ()>(&INT_KB, &[]).unwrap();
    let (_, mut ms) = interrupts.call_val::<1, _, ()>(&INT_MOUSE, &[]).unwrap();

    let kb_ev = Interrupt::from_handle(kb.pop().unwrap());
    let ms_ev = Interrupt::from_handle(ms.pop().unwrap());

    let kb_cbk = 1;
    let ms_cbk = 2;
    let kb_srv_cbk = 3;
    let ms_srv_cbk = 4;

    let (kb_service, kb_right) = Channel::new();
    kb_right.handle().publish("INPUT:KB");

    let (ms_service, ms_right) = Channel::new();
    ms_right.handle().publish("INPUT:MOUSE");

    let port = Port::new();

    kb_ev.set_port(&port, kb_cbk).assert_ok();
    ms_ev.set_port(&port, ms_cbk).assert_ok();

    ps2_controller.flush();

    println!("PS2 Ready");

    kb_service
        .handle()
        .wait_port(&port, ObjectSignal::READABLE, kb_srv_cbk)
        .assert_ok();
    ms_service
        .handle()
        .wait_port(&port, ObjectSignal::READABLE, ms_srv_cbk)
        .assert_ok();

    let mut kb_listeners: Vec<Channel> = Vec::new();
    let mut ms_listeners: Vec<Channel> = Vec::new();

    loop {
        let ev = port.wait().unwrap();

        if ev.key == kb_cbk {
            if let Some(ev) = ps2_controller.keyboard.check_interrupts() {
                let message = kernel_userspace::input::InputServiceMessage::KeyboardEvent(ev);
                kb_listeners.retain(|l| l.write_val(&message, &[]) == SyscallResult::Ok);
            }
            kb_ev.acknowledge().assert_ok();
        } else if ev.key == ms_cbk {
            if let Some(message) = ps2_controller.mouse.check_interrupts() {
                ms_listeners.retain(|l| l.write_val(&message, &[]) == SyscallResult::Ok);
            }
            ms_ev.acknowledge().assert_ok();
        } else if ev.key == kb_srv_cbk {
            let (_, mut handles) = kb_service.read_val::<1, bool>(false).unwrap();
            kb_listeners.push(Channel::from_handle(handles.pop().unwrap()));
        } else if ev.key == ms_srv_cbk {
            let (_, mut handles) = ms_service.read_val::<1, bool>(false).unwrap();
            ms_listeners.push(Channel::from_handle(handles.pop().unwrap()));
        }
    }
}
pub struct PS2Command {
    data_port: x86_64::instructions::port::Port<u8>,
    status_port: PortReadOnly<u8>,
    command_port: PortWriteOnly<u8>,
}

impl PS2Command {
    pub const fn new() -> Self {
        Self {
            data_port: x86_64::instructions::port::Port::new(0x60),
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
    sys_exit()
}
