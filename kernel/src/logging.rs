use core::fmt::Write;

use alloc::fmt;
use log::{Level, Log};

use crate::{screen::gop::WRITER, serial::SERIAL};

pub static KERNEL_LOGGER: KernelLogger = KernelLogger;
pub struct KernelLogger;

impl Log for KernelLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            print_log(record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn print_log(level: Level, target: &str, args: &fmt::Arguments) {
    if let Some(serial) = SERIAL.get() {
        serial
            .lock()
            .write_fmt(format_args!(
                "\x1b[1;{}m{: <5}\x1b[22;39m {} > {}\n",
                get_8bit_color_for_level(level),
                level,
                target,
                args
            ))
            .unwrap();
    }
    if let Some(w) = WRITER.get() {
        let mut w = w.lock();
        let color = w.tty.set_fg_colour(get_color_for_level(level));
        w.write_fmt(format_args!("{: <5} ", level)).unwrap();
        w.tty.set_fg_colour(0xFFFFFF);
        w.write_fmt(format_args!("{} > {}\n", target, args))
            .unwrap();
        w.tty.set_fg_colour(color);
    }
}

pub fn get_color_for_level(level: Level) -> u32 {
    match level {
        Level::Error => 0xFF5555,
        Level::Warn => 0xFFFF55,
        Level::Info => 0x55FF55,
        Level::Debug => 0x5555FF,
        Level::Trace => 0x55FFFF,
    }
}

pub fn get_8bit_color_for_level(level: Level) -> &'static str {
    match level {
        Level::Error => "31",
        Level::Warn => "33",
        Level::Info => "32",
        Level::Debug => "34",
        Level::Trace => "35",
    }
}
