use alloc::vec::Vec;
use kernel_sys::syscall::{sys_message_create, sys_message_read, sys_message_size};

use crate::handle::Handle;

/// This is a kernel ref counted immutable object
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHandle(Handle);

impl MessageHandle {
    pub const fn from_handle(id: Handle) -> Self {
        Self(id)
    }

    pub const fn handle(&self) -> &Handle {
        &self.0
    }

    pub fn into_inner(self) -> Handle {
        let Self(handle) = self;
        handle
    }

    pub fn create(data: &[u8]) -> Self {
        unsafe { Self(Handle::from_id(sys_message_create(data))) }
    }

    pub fn get_size(&self) -> usize {
        sys_message_size(*self.0).unwrap()
    }

    pub fn read(&self, buffer: &mut [u8]) {
        sys_message_read(*self.0, buffer).assert_ok();
    }

    pub fn read_vec(&self) -> Vec<u8> {
        let size = self.get_size();
        let mut vec = vec![0; size];
        self.read(&mut vec);
        vec
    }

    pub fn read_into_vec(&self, vec: &mut Vec<u8>) {
        let size = self.get_size();
        vec.resize(size, 0);
        self.read(vec);
    }
}
