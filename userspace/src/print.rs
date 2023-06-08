use core::fmt::{Arguments, Write};

use kernel_userspace::{
    fs::{stat_file, ReadRequest, ReadResponse, StatResponse, FS_STAT},
    service::{
        generate_tracking_number, get_public_service_id, get_service_messages_sync,
        send_and_get_response_sync, MessageType, SendMessageHeader, ServiceRequestServiceID,
        ServiceRequestServiceIDResponse, SID,
    },
    syscall::{service_push_msg, service_subscribe, spawn_thread, yield_now},
};

use spin::Mutex;

pub struct Writer {}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer {});

lazy_static::lazy_static! {
    pub static ref STDOUT: SID = get_public_service_id("STDOUT").unwrap();
}

impl Writer {
    pub fn write_byte(&mut self, chr: char) {
        let write = send_and_get_response_sync(
            *STDOUT,
            MessageType::Request,
            generate_tracking_number(),
            0,
            chr,
            0,
        );
        assert!(write.get_data_as::<bool>().unwrap())
    }

    pub fn write_string(&mut self, s: &str) {
        let write = send_and_get_response_sync(
            *STDOUT,
            MessageType::Request,
            generate_tracking_number(),
            0,
            s,
            0,
        );
        assert!(write.get_data_as::<bool>().unwrap())
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
