#![no_std]

extern crate alloc;

pub mod logger;

#[cfg(feature = "console")]
pub mod print;

use kernel_userspace::sys::syscall::sys_exit;
pub use log;

#[macro_export]
macro_rules! init_userspace {
    ($main:ident) => {
        #[unsafe(no_mangle)]
        #[unsafe(naked)]
        pub extern "C" fn _start() {
            extern "C" fn _start_inner() {
                ::userspace::log::set_logger(&::userspace::logger::USERSPACE_LOGGER).unwrap();
                ::userspace::log::set_max_level(::userspace::log::LevelFilter::Debug);

                $main();

                ::kernel_userspace::sys::syscall::sys_exit()
            }
            // We can't hit start directly, as we need to maintain the 16 byte alignment of the ABI
            core::arch::naked_asm!(
                "call {}",
                sym _start_inner,
            );
        }
    };
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "console")]
    {
        use core::fmt::Write;
        use print::WRITER_STDERR;

        if WRITER_STDERR
            .lock()
            .write_fmt(format_args!("{i}\n"))
            .is_err()
        {
            log::error!("Failed to write error message to stderr `{i}`");
        };
    }

    #[cfg(not(feature = "console"))]
    log::error!("{i}");

    sys_exit()
}
