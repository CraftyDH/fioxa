use core::fmt::{Arguments, Write};

use kernel_userspace::stream::{StreamMessage, StreamMessageType};
use kernel_userspace::syscall::{self, stream_push, STREAM_GETID_SOUT};
use spin::Mutex;

pub struct Writer {}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer {});

lazy_static::lazy_static! {
    pub static ref SOUT_ID: u64 = syscall::stream_get_id(STREAM_GETID_SOUT) as u64;
}

impl Writer {
    pub fn write_byte(&mut self, chr: char) {
        let mut data: [u8; 16] = [0u8; 16];
        data[0] = chr.len_utf8().try_into().unwrap();
        chr.encode_utf8(&mut data[1..]);
        let message = StreamMessage {
            stream_id: *SOUT_ID,
            message_type: StreamMessageType::InlineData,
            timestamp: 0,
            data,
        };
        stream_push(message);
    }

    pub fn write_string(&mut self, s: &str) {
        let mut chunks = s.as_bytes().chunks(15);
        while let Some(c) = chunks.next() {
            let mut data = [0u8; 16];
            data[0] = c.len().try_into().unwrap();
            data[1..1 + c.len()].copy_from_slice(c);
            let message: StreamMessage = StreamMessage {
                stream_id: *SOUT_ID,
                message_type: StreamMessageType::InlineData,
                timestamp: 0,
                data,
            };
            stream_push(message);
        }
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
