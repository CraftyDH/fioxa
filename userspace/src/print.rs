use core::fmt::{Arguments, Write};

use alloc::vec;
use kernel_userspace::{
    service::{
        generate_tracking_number, get_public_service_id, send_service_message, MessageType,
        ServiceResponse, SID,
    },
    syscall::{poll_service, service_get_data, yield_now},
};

use spin::Mutex;

pub struct Writer {
    pub pending_response: u8,
}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer {
    pending_response: 0,
});

lazy_static::lazy_static! {
    pub static ref STDOUT: SID = get_public_service_id("STDOUT").unwrap();
}

impl Writer {
    // Poll writes results later so that we can send multiple packets and not require as many round trips to send
    pub fn poll_errors(&mut self) {
        loop {
            while let Some(msg) = poll_service(*STDOUT, u64::MAX) {
                let mut data_buf = vec![0u8; msg.data_length];
                service_get_data(&mut data_buf).unwrap();
                let write = ServiceResponse::new(msg, data_buf);
                assert!(write.get_data_as::<bool>().unwrap());
                self.pending_response -= 1;
            }

            if self.pending_response > 100 {
                yield_now()
            } else {
                break;
            }
        }
    }
    pub fn write_byte(&mut self, chr: char) {
        self.poll_errors();
        self.pending_response += 1;
        send_service_message(
            *STDOUT,
            MessageType::Request,
            generate_tracking_number(),
            0,
            chr,
            0,
        );
    }

    pub fn write_string(&mut self, s: &str) {
        self.poll_errors();
        self.pending_response += 1;
        send_service_message(
            *STDOUT,
            MessageType::Request,
            generate_tracking_number(),
            0,
            s,
            0,
        );
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
