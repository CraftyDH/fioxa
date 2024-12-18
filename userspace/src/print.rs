use core::fmt::{Arguments, Write};

use alloc::vec::Vec;
use kernel_userspace::{
    channel::{channel_read_rs, channel_write_rs, ChannelReadResult},
    object::{object_wait, KernelReferenceID, ObjectSignal},
    process::get_handle,
    syscall::exit,
};

use spin::{Lazy, Mutex};

pub struct Writer {
    stdout_socket: KernelReferenceID,
    in_flight: usize,
}

pub static WRITER: Lazy<Mutex<Writer>> = Lazy::new(|| {
    let handle = get_handle("STDOUT").unwrap();

    Mutex::new(Writer {
        stdout_socket: handle,
        in_flight: 0,
    })
});

impl Writer {
    pub fn write_raw(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(0x1000) {
            if self.in_flight > 100 {
                let mut data = Vec::new();
                let mut handles = Vec::new();
                loop {
                    match channel_read_rs(self.stdout_socket, &mut data, &mut handles) {
                        ChannelReadResult::Ok => break,
                        ChannelReadResult::Empty => {
                            object_wait(self.stdout_socket, ObjectSignal::READABLE);
                            continue;
                        }
                        _ => exit(),
                    }
                }
            }
            channel_write_rs(self.stdout_socket, chunk, &[]);
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
