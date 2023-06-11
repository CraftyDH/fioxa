#![no_std]
#![feature(error_in_core)]

#[macro_use]
extern crate alloc;

pub mod disk;
pub mod fs;
pub mod ids;
pub mod input;
pub mod proc;
pub mod service;
pub mod syscall;
