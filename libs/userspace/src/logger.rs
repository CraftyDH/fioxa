use core::fmt::Write;

use alloc::string::String;
use kernel_userspace::sys::syscall::sys_log;
use log::Log;
use spin::mutex::Mutex;

pub static USERSPACE_LOGGER: UserspaceLogger = UserspaceLogger {
    str_buffer: Mutex::new(String::new()),
};

pub struct UserspaceLogger {
    str_buffer: Mutex<String>,
}

impl Log for UserspaceLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let mut str_buffer = self.str_buffer.lock();
            str_buffer.clear();
            str_buffer.write_fmt(*record.args()).unwrap();
            sys_log(record.level() as u32, record.target(), &str_buffer);
        }
    }

    fn flush(&self) {}
}
