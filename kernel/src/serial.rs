use core::fmt::Write;

use alloc::{string::String, vec::Vec};
use kernel_sys::syscall::{sys_exit, sys_process_spawn_thread};
use kernel_userspace::{
    channel::Channel,
    handle::Handle,
    interrupt::InterruptsService,
    ipc::IPCChannel,
    process::{INIT_HANDLE_SERVICE, ProcessHandle},
};
use spin::Once;
use x86_64::instructions::{interrupts::without_interrupts, port::Port};

use crate::{
    bootfs::TERMINAL_ELF, cpu_localstorage::CPULocalStorageRW, elf::load_elf, mutex::Spinlock,
    scheduling::process::ProcessReferences,
};

pub static SERIAL: Once<Spinlock<Serial>> = Once::new();

pub const COM_1: u16 = 0x3f8;

pub struct Serial {
    bus_base: u16,
}

/// Code based on https://wiki.osdev.org/Serial_Ports
impl Serial {
    pub const fn new(bus_base: u16) -> Self {
        Self { bus_base }
    }

    fn get_port(&mut self, offset: u16) -> Port<u8> {
        Port::new(self.bus_base + offset)
    }

    pub unsafe fn init(&mut self) -> bool {
        without_interrupts(|| unsafe {
            // Disable all interrupts
            self.get_port(1).write(0x00);

            // Enable DLAB (set baurd rate divisor)
            self.get_port(3).write(0x80);

            // Set baud rate to 9600
            self.get_port(0).write((115200 / 9600) as u8);
            self.get_port(1).write(0x00);

            // set 8 bits no parity, one stop bit
            self.get_port(3).write(0x03);

            // Enable FIFO, clear them, with 14 byte threshold
            self.get_port(2).write(0xC7);

            // IRQ's enabled, RTS/DSR set
            self.get_port(4).write(0x0B);

            // Set loopback mode, test the chip
            self.get_port(4).write(0x1E);

            // Test serial by sending byte
            self.get_port(0).write(0xAE);

            if self.get_port(0).read() != 0xAE {
                error!("Serial test failed");
                return false;
            }

            // Set normal operation mode
            // (not-loopback with IRQs enabled and OUT#1 and OUT#2 bits enabled)
            self.get_port(4).write(0x0F);

            // Enable interrupts for readable
            self.get_port(1).write(0b1);

            true
        })
    }

    pub fn readable(&mut self) -> bool {
        unsafe { self.get_port(5).read() & 1 > 0 }
    }

    pub fn read_serial(&mut self) -> u8 {
        unsafe {
            while !self.readable() {
                core::hint::spin_loop();
            }

            self.get_port(0).read()
        }
    }

    pub fn try_read(&mut self) -> Option<u8> {
        unsafe { self.readable().then(|| self.get_port(0).read()) }
    }

    pub fn writeable(&mut self) -> bool {
        unsafe { self.get_port(5).read() & 0x20 > 0 }
    }

    pub fn write_serial(&mut self, byte: u8) {
        unsafe {
            while !self.writeable() {
                core::hint::spin_loop();
            }

            self.get_port(0).write(byte)
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_serial(b);
        }
    }
}

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_str(s);
        Result::Ok(())
    }
}

pub fn serial_monitor_stdin() {
    let Some(serial) = SERIAL.get() else {
        warn!("Serial device not found");
        sys_exit();
    };
    let comm1 = InterruptsService::from_channel(IPCChannel::connect("INTERRUPTS"))
        .get_interrupt(kernel_userspace::interrupt::InterruptVector::COM1)
        .unwrap();

    let (stdin, cin) = Channel::new();
    let (stdout, cout) = Channel::new();

    sys_process_spawn_thread(move || {
        loop {
            let proc = load_elf(TERMINAL_ELF)
                .unwrap()
                .references(ProcessReferences::from_refs(&[
                    **INIT_HANDLE_SERVICE.lock().clone_init_service().handle(),
                    **cin.handle(),
                    **cout.handle(),
                    **cout.handle(),
                ]))
                .build();

            let mut proc = unsafe {
                let thread = CPULocalStorageRW::get_current_task();
                ProcessHandle::from_handle(Handle::from_id(thread.process().add_value(proc.into())))
            };

            proc.blocking_exit_code();
            warn!("Terminal exited")
        }
    });

    sys_process_spawn_thread(move || {
        let serial = SERIAL.get().unwrap();
        let mut read = Vec::with_capacity(0x1000);
        loop {
            stdout.read::<0>(&mut read, false, true).unwrap();
            let s = String::from_utf8_lossy(&read);
            let mut serial = serial.lock();
            for c in s.chars() {
                if c == '\n' {
                    serial.write_str("\r\n");
                } else if c == '\x08' {
                    // go back, write space, go back
                    serial.write_str("\x08 \x08");
                } else {
                    serial.write_char(c).unwrap();
                }
            }
        }
    });

    loop {
        while let Some(b) = { serial.lock().try_read() } {
            if b == b'\r' {
                stdin.write(b"\n", &[]).assert_ok();
            } else if b == 127 {
                // delete character, make it backspace
                stdin.write(&[8], &[]).assert_ok();
            } else {
                stdin.write(&[b], &[]).assert_ok();
            }
        }
        comm1.wait().assert_ok();
    }
}
