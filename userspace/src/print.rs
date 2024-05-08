use core::fmt::{Arguments, Write};

use kernel_userspace::{
    message::MessageHandle,
    object::{KernelReference, REFERENCE_STDOUT},
    socket::SocketHandle,
};

use spin::{Lazy, Mutex};

pub struct Writer {
    stdout_socket: SocketHandle,
}

pub static WRITER: Lazy<Mutex<Writer>> = Lazy::new(|| {
    Mutex::new(Writer {
        stdout_socket: SocketHandle::from_raw_socket(KernelReference::from_id(REFERENCE_STDOUT)),
    })
});

impl Writer {
    pub fn write_string(&mut self, s: &str) {
        let msg = MessageHandle::create(s.as_bytes());
        self.stdout_socket.blocking_send(msg.kref()).unwrap();
    }
}

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_string(s);
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
