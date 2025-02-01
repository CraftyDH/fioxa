use core::fmt::Write;

use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use kernel_sys::syscall::sys_exit;
use kernel_userspace::{
    backoff_sleep, channel::Channel, interrupt::Interrupt, process::get_handle, INT_COM1,
};
use log::LevelFilter;
use x86_64::instructions::{interrupts::without_interrupts, port::Port};

use crate::{
    mutex::Spinlock,
    scheduling::taskmanager::{PROCESSES, SCHEDULER},
    time::{uptime, SLEPT_PROCESSES},
};

pub static SERIAL: OnceCell<Spinlock<Serial>> = OnceCell::uninit();

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
        without_interrupts(|| {
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
    let ints = Channel::from_handle(backoff_sleep(|| get_handle("INTERRUPTS")));
    let mut handles_buf = Vec::with_capacity(1);
    let (_, mut handles) = ints
        .call_val::<1, _, ()>(&INT_COM1, &mut handles_buf)
        .unwrap();

    let ints = Interrupt::from_handle(handles.pop().unwrap());

    loop {
        let mut serial = serial.lock();
        while let Some(b) = serial.try_read() {
            let c: char = b.into();

            match c {
                '\r' => serial.write_serial(b'\n'),
                's' => {
                    SCHEDULER.lock().dump_runnable(&mut *serial).unwrap();

                    serial
                        .write_fmt(format_args!("Slept processes (time: {})\n", uptime()))
                        .unwrap();
                    for slept in SLEPT_PROCESSES.lock().iter() {
                        serial
                            .write_fmt(format_args!("{} {:?}\n", slept.0.wakeup, slept.0.thread))
                            .unwrap();
                    }
                }
                'l' => {
                    serial.write_str("Change log level to: ");
                    let to = serial.read_serial();
                    let to = match to {
                        b'e' => LevelFilter::Error,
                        b'w' => LevelFilter::Warn,
                        b'i' => LevelFilter::Info,
                        b'd' => LevelFilter::Debug,
                        b't' => LevelFilter::Trace,
                        _ => {
                            serial.write_serial(to);
                            serial.write_str(" Unknown log level\n");
                            continue;
                        }
                    };
                    log::set_max_level(to);
                    serial
                        .write_fmt(format_args!("Set log level to {to}\n"))
                        .unwrap();
                }
                'p' => {
                    let processes = PROCESSES.lock();

                    for proc in processes.iter() {
                        serial
                            .write_fmt(format_args!("{:?} {}\n", proc.0, proc.1.name))
                            .unwrap();

                        for thread in proc.1.threads.lock().threads.iter() {
                            serial
                                .write_fmt(format_args!("\t{:?}\n", thread.1))
                                .unwrap();
                        }
                    }
                }
                _ => serial
                    .write_fmt(format_args!("Unknown cmd: {c}\n"))
                    .unwrap(),
            }
        }
        drop(serial);
        ints.wait().assert_ok();
    }
}
