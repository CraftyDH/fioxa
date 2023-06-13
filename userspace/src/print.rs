use core::fmt::{Arguments, Write};

use kernel_userspace::{
    ids::ServiceID,
    service::{
        generate_tracking_number, get_public_service_id, SendServiceMessageDest, ServiceMessage,
        ServiceMessageType,
    },
    syscall::{send_service_message, try_receive_service_message, yield_now, CURRENT_PID},
};

use spin::Mutex;

pub struct Writer {
    pub pending_response: u8,
}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer {
    pending_response: 0,
});

lazy_static::lazy_static! {
    pub static ref STDOUT: ServiceID = get_public_service_id("STDOUT").unwrap();
}

impl Writer {
    // Poll writes results later so that we can send multiple packets and not require as many round trips to send
    pub fn poll_errors(&mut self) {
        loop {
            while let Some(msg) = try_receive_service_message(*STDOUT) {
                let message = msg.get_message().unwrap();
                match message.message {
                    ServiceMessageType::Ack => {
                        self.pending_response -= 1;
                    }
                    _ => todo!(),
                }
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
        send_service_message(&ServiceMessage {
            service_id: *STDOUT,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::StdoutChar(chr),
        })
        .unwrap();
    }

    pub fn write_string(&mut self, s: &str) {
        self.poll_errors();
        self.pending_response += 1;
        send_service_message(&ServiceMessage {
            service_id: *STDOUT,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::Stdout(s),
        })
        .unwrap();
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
