use alloc::boxed::Box;

use crate::{paging::MemoryLoc, scheduling::process::Process};

#[derive(Debug, Clone, Copy)]
pub struct UserPtrBounds(usize);

impl UserPtrBounds {
    pub fn top(self) -> usize {
        self.0
    }
}

/// Gets the upperbound of virtual memory that the process should be allowed to ask a syscall to read/write
pub fn get_current_bounds(process: &Process) -> UserPtrBounds {
    match process.privilege {
        crate::scheduling::process::ProcessPrivilege::KERNEL => UserPtrBounds(usize::MAX),
        crate::scheduling::process::ProcessPrivilege::USER => {
            // make it inclusive
            UserPtrBounds(MemoryLoc::EndUserMem as usize + 1)
        }
    }
}

pub struct UserPtr<T>(*const T);

pub struct UserPtrMut<T>(*mut T);

impl<T: Copy> UserPtr<T> {
    pub unsafe fn new(ptr: *const T, bounds: UserPtrBounds) -> Option<Self> {
        if ptr as usize + size_of::<T>() > bounds.0 {
            return None;
        }
        Some(Self(ptr))
    }

    pub fn read(&self) -> T {
        unsafe { *self.0 }
    }
}

impl<T: Copy> UserPtrMut<T> {
    pub unsafe fn new(ptr: *mut T, bounds: UserPtrBounds) -> Option<Self> {
        if ptr as usize + size_of::<T>() > bounds.0 {
            return None;
        }
        Some(Self(ptr))
    }

    pub fn read(&self) -> T {
        unsafe { *self.0 }
    }

    pub fn write(&mut self, val: T) {
        unsafe { *self.0 = val }
    }
}

pub struct UserBytes {
    ptr: *const u8,
    len: usize,
}

impl UserBytes {
    pub unsafe fn new(ptr: *const u8, len: usize, bounds: UserPtrBounds) -> Option<Self> {
        if ptr as usize + len > bounds.0 {
            return None;
        }

        Some(Self { ptr, len })
    }

    pub fn read(&self, buf: &mut [u8]) {
        assert_eq!(buf.len(), self.len);
        unsafe {
            let slice = core::slice::from_raw_parts(self.ptr, self.len);
            buf.copy_from_slice(slice);
        }
    }

    pub fn read_to_box(&self) -> Box<[u8]> {
        unsafe {
            let slice = core::slice::from_raw_parts(self.ptr, self.len);
            slice.into()
        }
    }
}

pub struct UserBytesMut {
    ptr: *mut u8,
    len: usize,
}

impl UserBytesMut {
    pub unsafe fn new(ptr: *mut u8, len: usize, bounds: UserPtrBounds) -> Option<Self> {
        if ptr as usize + len > bounds.0 {
            return None;
        }

        Some(Self { ptr, len })
    }

    pub fn read(&self, buf: &mut [u8]) {
        assert_eq!(buf.len(), self.len);
        unsafe {
            let slice = core::slice::from_raw_parts(self.ptr, self.len);
            buf.copy_from_slice(slice);
        }
    }

    pub fn read_to_box(&self) -> Box<[u8]> {
        unsafe {
            let slice = core::slice::from_raw_parts(self.ptr, self.len);
            slice.into()
        }
    }

    pub fn write(&mut self, buf: &[u8]) {
        assert_eq!(buf.len(), self.len);
        unsafe {
            let slice = core::slice::from_raw_parts_mut(self.ptr, self.len);
            slice.copy_from_slice(buf);
        }
    }
}
