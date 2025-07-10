use kernel_sys::{
    syscall::{sys_port_create, sys_port_push, sys_port_wait},
    types::{SysPortNotification, SyscallResult},
};

use crate::handle::Handle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port(Handle);

impl Default for Port {
    fn default() -> Self {
        Self::new()
    }
}

impl Port {
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

    pub fn new() -> Self {
        unsafe { Port(Handle::from_id(sys_port_create())) }
    }

    pub fn wait(&self) -> Result<SysPortNotification, SyscallResult> {
        sys_port_wait(*self.0)
    }

    pub fn push(&self, notification: &SysPortNotification) -> SyscallResult {
        sys_port_push(*self.0, notification)
    }
}
