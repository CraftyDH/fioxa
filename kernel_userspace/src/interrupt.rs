use kernel_sys::{
    syscall::{
        sys_interrupt_acknowledge, sys_interrupt_create, sys_interrupt_set_port,
        sys_interrupt_trigger, sys_interrupt_wait,
    },
    types::SyscallResult,
};

use crate::{handle::Handle, port::Port};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupt(Handle);

impl Interrupt {
    pub const fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }

    pub const fn handle(&self) -> &Handle {
        &self.0
    }

    pub fn into_inner(self) -> Handle {
        let Self(handle) = self;
        handle
    }

    pub fn new() -> Interrupt {
        unsafe { Self::from_handle(Handle::from_id(sys_interrupt_create())) }
    }

    pub fn wait(&self) -> SyscallResult {
        sys_interrupt_wait(*self.0)
    }

    pub fn trigger(&self) -> SyscallResult {
        sys_interrupt_trigger(*self.0)
    }

    pub fn acknowledge(&self) -> SyscallResult {
        sys_interrupt_acknowledge(*self.0)
    }

    pub fn set_port(&self, port: &Port, key: u64) -> SyscallResult {
        sys_interrupt_set_port(*self.0, **port.handle(), key)
    }
}
