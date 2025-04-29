use alloc::vec::Vec;
use kernel_sys::{
    syscall::{
        sys_channel_create, sys_channel_read_val, sys_channel_read_vec, sys_channel_write,
        sys_channel_write_val,
    },
    types::{Hid, SyscallResult},
};
use rkyv::{Archive, Deserialize, Serialize};

use crate::handle::Handle;

#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct Channel(Handle);

impl Channel {
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

    pub fn new() -> (Channel, Channel) {
        unsafe {
            let (left, right) = sys_channel_create();
            let left = Self::from_handle(Handle::from_id(left));
            let right = Self::from_handle(Handle::from_id(right));

            (left, right)
        }
    }

    pub fn read<const N: usize>(
        &self,
        buf: &mut Vec<u8>,
        resize: bool,
        blocking: bool,
    ) -> Result<heapless::Vec<Handle, N>, SyscallResult> {
        let handles = sys_channel_read_vec::<N>(*self.0, buf, resize, blocking)?;
        // Safety: The kernel will return new handles
        let handles = handles
            .into_iter()
            .map(|h| unsafe { Handle::from_id(h) })
            .collect();
        Ok(handles)
    }

    pub fn read_val<const N: usize, V: Sized>(
        &self,
        blocking: bool,
    ) -> Result<(V, heapless::Vec<Handle, N>), SyscallResult> {
        let (v, handles) = sys_channel_read_val::<V, N>(*self.0, blocking)?;
        // Safety: The kernel will return new handles
        let handles = handles
            .into_iter()
            .map(|h| unsafe { Handle::from_id(h) })
            .collect();
        Ok((v, handles))
    }

    pub fn write(&self, buf: &[u8], handles: &[Hid]) -> SyscallResult {
        sys_channel_write(*self.0, buf, handles)
    }

    pub fn write_val<V: Sized>(&self, val: &V, handles: &[Hid]) -> SyscallResult {
        sys_channel_write_val(*self.0, val, handles)
    }

    pub fn call<const N: usize>(
        &self,
        buf: &mut Vec<u8>,
        handles: &[Hid],
    ) -> Result<heapless::Vec<Handle, N>, SyscallResult> {
        self.write(buf, handles).into_err()?;
        self.read(buf, true, true)
    }

    pub fn call_val<const N: usize, S, R>(
        &self,
        val: &S,
        handles: &[Hid],
    ) -> Result<(R, heapless::Vec<Handle, N>), SyscallResult> {
        self.write_val(val, handles).into_err()?;
        self.read_val(true)
    }
}
