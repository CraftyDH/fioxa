use core::ops::Deref;

use kernel_sys::{
    syscall::{sys_handle_clone, sys_handle_drop, sys_object_wait, sys_object_wait_port},
    types::{Hid, ObjectSignal},
};

use crate::port::Port;

/// A struct that owns the handle and will free it on drop
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Handle(Hid);

pub const FIRST_HANDLE: Handle = unsafe { Handle::from_id(Hid::from_usize(1).unwrap()) };

impl Handle {
    /// Construct a handle from an Hid
    ///
    /// # Safety
    ///
    /// This handle will now own Hid no other references to it can outlive it.
    pub const unsafe fn from_id(id: Hid) -> Self {
        Self(id)
    }

    pub fn wait(
        &self,
        signal: ObjectSignal,
    ) -> Result<ObjectSignal, kernel_sys::types::SyscallError> {
        sys_object_wait(self.0, signal)
    }

    pub fn wait_port(
        &self,
        port: &Port,
        on: ObjectSignal,
        key: u64,
    ) -> Result<(), kernel_sys::types::SyscallError> {
        sys_object_wait_port(self.0, port.handle().0, on, key)
    }
}

impl Deref for Handle {
    type Target = Hid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { sys_handle_drop(self.0).unwrap() }
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        Self(sys_handle_clone(self.0).unwrap())
    }
}
