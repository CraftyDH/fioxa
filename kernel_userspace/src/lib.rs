#![no_std]

#[macro_use]
extern crate alloc;

pub mod fs;
pub mod proc;
pub mod service;
pub mod syscall;

pub type SOUT_WRITE_LINE<'a> = &'a str;
pub type SOUT_WRITE_LINE_RESP = bool;
