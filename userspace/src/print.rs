use core::fmt::{Arguments, Write};

use alloc::vec::Vec;
use kernel_userspace::{channel::Channel, process::get_handle};

use spin::{Lazy, Mutex};

pub struct Writer {
    stdout_socket: Channel,
    in_flight: usize,
}

pub static WRITER: Lazy<Mutex<Writer>> = Lazy::new(|| {
    let handle = get_handle("STDOUT").unwrap();

    Mutex::new(Writer {
        stdout_socket: Channel::from_handle(handle),
        in_flight: 0,
    })
});

impl Writer {
    pub fn write_raw(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(0x1000) {
            if self.in_flight > 100 {
                let mut data = Vec::new();

                self.stdout_socket
                    .read::<0>(&mut data, false, true)
                    .unwrap();
            }
            self.stdout_socket.write(chunk, &[]).assert_ok();
        }
    }
}

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_raw(s.as_bytes());
        Ok(())
    }
}

#[macro_export]
macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::print::_print(format_args!($($arg)*)));
}

pub fn _print(args: Arguments) {
    WRITER.lock().write_fmt(args).unwrap();
}
