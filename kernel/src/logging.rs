use core::fmt::Write;

use log::{Level, Log};

use crate::{scheduling::with_held_interrupts, screen::gop::WRITER};

pub static KERNEL_LOGGER: KernelLogger = KernelLogger;
pub struct KernelLogger;

impl Log for KernelLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            with_held_interrupts(|| {
                let mut w = WRITER.get().unwrap().lock();
                let color = w.tty.set_fg_colour(get_color_for_level(record.level()));
                w.write_fmt(format_args!("{} ", record.level())).unwrap();
                w.tty.set_fg_colour(0xFFFFFF);
                w.write_fmt(format_args!("{} > {}\n", record.target(), record.args()))
                    .unwrap();
                w.tty.set_fg_colour(color);
            });
        }
    }

    fn flush(&self) {}
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
