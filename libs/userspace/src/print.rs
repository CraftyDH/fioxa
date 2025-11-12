use core::{
    fmt::{Arguments, Write},
    time::Duration,
};

use kernel_userspace::{
    channel::Channel,
    handle::Handle,
    sys::{syscall::sys_sleep, types::Hid},
};

use log::warn;
use spin::{Lazy, Mutex};

pub struct Writer<'a> {
    channel: &'a Channel,
}

pub static STDIN_CHANNEL: Channel =
    Channel::from_handle(unsafe { Handle::from_id(Hid::from_usize(2).unwrap()) });
pub static STDOUT_CHANNEL: Channel =
    Channel::from_handle(unsafe { Handle::from_id(Hid::from_usize(3).unwrap()) });
pub static STDERR_CHANNEL: Channel =
    Channel::from_handle(unsafe { Handle::from_id(Hid::from_usize(4).unwrap()) });

pub static WRITER_STDOUT: Lazy<Mutex<Writer<'static>>> = Lazy::new(|| {
    Mutex::new(Writer {
        channel: &STDOUT_CHANNEL,
    })
});

pub static WRITER_STDERR: Lazy<Mutex<Writer<'static>>> = Lazy::new(|| {
    Mutex::new(Writer {
        channel: &STDERR_CHANNEL,
    })
});

impl Writer<'_> {
    pub fn write_raw(&mut self, bytes: &[u8]) -> core::fmt::Result {
        for chunk in bytes.chunks(0x1000) {
            let mut sleep = 1;
            loop {
                match self.channel.write(chunk, &[]) {
                    Ok(()) => break,
                    Err(kernel_userspace::sys::types::SyscallError::ChannelClosed) => {
                        return Ok(());
                    }
                    Err(kernel_userspace::sys::types::SyscallError::ChannelFull) => {
                        sys_sleep(Duration::from_millis(sleep));
                        sleep = 1000.max(sleep * 2);
                    }
                    _ => {
                        warn!("Failed to write to stdout/stderr");
                        return Err(core::fmt::Error);
                    }
                }
            }
        }
        return Ok(());
    }
}

impl core::fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_raw(s.as_bytes())
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
    WRITER_STDOUT.lock().write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! eprintln {
    () => (eprint!("\n"));
    ($($arg:tt)*) => (eprint!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => ($crate::print::_eprint(format_args!($($arg)*)));
}

pub fn _eprint(args: Arguments) {
    WRITER_STDERR.lock().write_fmt(args).unwrap();
}
