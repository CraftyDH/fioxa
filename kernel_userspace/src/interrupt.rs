use core::u64;

use num_derive::{FromPrimitive, ToPrimitive};

use crate::{make_syscall, object::KernelReferenceID};

#[derive(FromPrimitive, ToPrimitive)]
pub enum InterruptSyscall {
    Create,
    Trigger,
    SetPort,
    Acknowledge,
    Wait,
}

pub fn interrupt_create() -> KernelReferenceID {
    let id: usize;
    unsafe { make_syscall!(crate::syscall::INTERRUPT, InterruptSyscall::Create as usize => id) };
    KernelReferenceID::from_usize(id).unwrap()
}

pub fn interrupt_trigger(handle: KernelReferenceID) {
    unsafe {
        make_syscall!(
            crate::syscall::INTERRUPT,
            InterruptSyscall::Trigger as usize,
            handle.0.get()
        )
    };
}

pub fn interrupt_set_port(handle: KernelReferenceID, port: KernelReferenceID, key: u64) {
    unsafe {
        let _x: u16;
        make_syscall!(
            crate::syscall::INTERRUPT,
            InterruptSyscall::SetPort as usize,
            handle.0.get(),
            port.0.get(),
            key => _x
        )
    };
}

pub fn interrupt_acknowledge(handle: KernelReferenceID) {
    unsafe {
        make_syscall!(
            crate::syscall::INTERRUPT,
            InterruptSyscall::Acknowledge as usize,
            handle.0.get()
        )
    };
}

pub fn interrupt_wait(handle: KernelReferenceID) {
    unsafe {
        make_syscall!(
            crate::syscall::INTERRUPT,
            InterruptSyscall::Wait as usize,
            handle.0.get()
        )
    };
}
