#![no_std]
#![no_main]

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use core::num::NonZeroUsize;

use alloc::vec::Vec;
use kernel_userspace::{
    event::{
        event_queue_create, event_queue_get_event, event_queue_listen, event_queue_pop,
        receive_event, EventCallback, KernelEventQueueListenMode, ReceiveMode,
    },
    object::{KernelObjectType, KernelReference},
    service::{make_message, make_message_new},
    socket::{socket_accept, socket_listen, socket_listen_get_event, socket_send, SocketHandle},
    syscall::{exit, sleep},
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

    let mut buffer = Vec::new();

    let interrupts = SocketHandle::connect("INTERRUPTS").unwrap();

    let kb_msg = make_message(&INT_KB, &mut buffer);
    let mouse_msg = make_message(&INT_MOUSE, &mut buffer);

    interrupts.blocking_send(kb_msg.kref()).unwrap();
    interrupts.blocking_send(mouse_msg.kref()).unwrap();

    let (kb_event, kb_ty) = interrupts.blocking_recv().unwrap();
    let (mouse_event, m_ty) = interrupts.blocking_recv().unwrap();

    assert_eq!(kb_ty, KernelObjectType::Event);
    assert_eq!(m_ty, KernelObjectType::Event);

    let event_queue = event_queue_create();
    let event_queue_event = event_queue_get_event(event_queue);

    let kb_cbk = EventCallback(NonZeroUsize::new(1).unwrap());
    let ms_cbk = EventCallback(NonZeroUsize::new(2).unwrap());
    let kb_srv_cbk = EventCallback(NonZeroUsize::new(3).unwrap());
    let ms_srv_cbk = EventCallback(NonZeroUsize::new(4).unwrap());

    let kb_service = socket_listen("INPUT:KB").unwrap();
    let kb_service_ev = socket_listen_get_event(kb_service);
    let ms_service = socket_listen("INPUT:MOUSE").unwrap();
    let ms_service_ev = socket_listen_get_event(ms_service);

    event_queue_listen(
        event_queue,
        kb_event.id(),
        kb_cbk,
        KernelEventQueueListenMode::OnEdgeHigh,
    );

    event_queue_listen(
        event_queue,
        mouse_event.id(),
        ms_cbk,
        KernelEventQueueListenMode::OnEdgeHigh,
    );

    // TODO: The rest of the kernel sped up and weird behaviour came up (mouse dying)
    // sleep to add delay :(
    sleep(500);
    ps2_controller.flush();
    while let Some(event) = event_queue_pop(event_queue) {
        println!("PS2 Flushing");
        if event == kb_cbk || event == ms_cbk {
            ps2_controller.flush();
        } else {
            panic!("unreachable")
        }
    }

    println!("PS2 Ready");

    event_queue_listen(
        event_queue,
        kb_service_ev,
        kb_srv_cbk,
        KernelEventQueueListenMode::OnLevelHigh,
    );

    event_queue_listen(
        event_queue,
        ms_service_ev,
        ms_srv_cbk,
        KernelEventQueueListenMode::OnLevelHigh,
    );

    let mut kb_listeners: Vec<KernelReference> = Vec::new();
    let mut ms_listeners: Vec<KernelReference> = Vec::new();

    loop {
        receive_event(event_queue_event, ReceiveMode::LevelHigh);
        while let Some(event) = event_queue_pop(event_queue) {
            if event == kb_cbk {
                if let Some(ev) = ps2_controller.keyboard.check_interrupts() {
                    let message = make_message_new(
                        &kernel_userspace::input::InputServiceMessage::KeyboardEvent(ev),
                    );
                    // ignore if pipe is full, just drop
                    kb_listeners.retain(|l| match socket_send(l.id(), message.kref().id()) {
                        Err(kernel_userspace::socket::SocketSendResult::Closed) => false,
                        _ => true,
                    });
                }
            } else if event == ms_cbk {
                if let Some(message) = ps2_controller.mouse.check_interrupts() {
                    // ignore if pipe is full, just drop
                    ms_listeners.retain(|l| match socket_send(l.id(), message.kref().id()) {
                        Err(kernel_userspace::socket::SocketSendResult::Closed) => false,
                        _ => true,
                    });
                }
            } else if event == kb_srv_cbk {
                if let Some(sock) = socket_accept(kb_service) {
                    kb_listeners.push(KernelReference::from_id(sock))
                }
            } else if event == ms_srv_cbk {
                if let Some(sock) = socket_accept(ms_service) {
                    ms_listeners.push(KernelReference::from_id(sock))
                }
            }
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
