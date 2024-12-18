use core::{mem::MaybeUninit, u64};

use alloc::vec::Vec;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;

use crate::{
    make_syscall,
    object::{delete_reference, object_wait, KernelReference, KernelReferenceID, ObjectSignal},
};

#[derive(FromPrimitive, ToPrimitive)]
pub enum ChannelSyscall {
    Create,
    Read,
    Write,
}

#[repr(C)]
pub struct ChannelCreate {
    pub left: Option<KernelReferenceID>,
    pub right: Option<KernelReferenceID>,
}

impl Default for ChannelCreate {
    fn default() -> Self {
        Self {
            left: Default::default(),
            right: Default::default(),
        }
    }
}

pub fn channel_create(create: &mut ChannelCreate) -> bool {
    unsafe {
        let res: u16;
        make_syscall!(
            crate::syscall::CHANNEL,
            ChannelSyscall::Create as usize,
            create => res);
        res != 0
    }
}

pub fn channel_create_rs() -> (KernelReference, KernelReference) {
    let mut create = ChannelCreate {
        left: None,
        right: None,
    };
    channel_create(&mut create);
    (
        KernelReference::from_id(create.left.unwrap()),
        KernelReference::from_id(create.right.unwrap()),
    )
}

#[repr(C)]
pub struct ChannelRead {
    pub handle: KernelReferenceID,
    pub data: *mut u8,
    pub data_len: usize,
    pub handles: *mut u8,
    pub handles_len: usize,
}

#[derive(Debug, FromPrimitive, ToPrimitive)]
pub enum ChannelReadResult {
    Ok,
    Empty,
    Size,
    Closed,
}

pub fn channel_read(read: &mut ChannelRead) -> ChannelReadResult {
    unsafe {
        let res: u16;
        make_syscall!(
            crate::syscall::CHANNEL,
            ChannelSyscall::Read as usize,
            read => res);
        ChannelReadResult::from_u16(res).unwrap()
    }
}

#[repr(C)]
pub struct ChannelWrite {
    pub handle: KernelReferenceID,
    pub data: *const u8,
    pub data_len: usize,
    pub handles: *const u8,
    pub handles_len: usize,
}

pub fn channel_write(write: &ChannelWrite) -> bool {
    unsafe {
        let res: u16;
        make_syscall!(
            crate::syscall::CHANNEL,
            ChannelSyscall::Write as usize,
            write => res);
        res != 0
    }
}

pub fn channel_write_rs(
    handle: KernelReferenceID,
    data: &[u8],
    handles: &[KernelReferenceID],
) -> bool {
    let write = ChannelWrite {
        handle,
        data: data.as_ptr(),
        data_len: data.len(),
        handles: handles.as_ptr().cast(),
        handles_len: handles.len(),
    };
    channel_write(&write)
}

pub fn channel_write_val<V>(
    handle: KernelReferenceID,
    data: &V,
    handles: &[KernelReferenceID],
) -> bool {
    let write = ChannelWrite {
        handle,
        data: data as *const V as *const u8,
        data_len: size_of::<V>(),
        handles: handles.as_ptr().cast(),
        handles_len: handles.len(),
    };
    channel_write(&write)
}

pub fn channel_read_rs(
    handle: KernelReferenceID,
    data: &mut Vec<u8>,
    handles: &mut Vec<KernelReferenceID>,
) -> ChannelReadResult {
    let mut read = ChannelRead {
        handle,
        data: data.as_mut_ptr(),
        data_len: data.capacity(),
        handles: handles.as_mut_ptr().cast(),
        handles_len: handles.capacity(),
    };

    loop {
        let res = channel_read(&mut read);
        match res {
            ChannelReadResult::Ok => unsafe {
                data.set_len(read.data_len);
                handles.set_len(read.handles_len);
                return res;
            },
            ChannelReadResult::Empty => {
                object_wait(handle, ObjectSignal::READABLE);
            }
            _ => unsafe {
                data.set_len(0);
                handles.set_len(0);
                return res;
            },
        }
    }
}

pub fn channel_read_resize(
    handle: KernelReferenceID,
    data: &mut Vec<u8>,
    handles: &mut Vec<KernelReferenceID>,
) -> ChannelReadResult {
    loop {
        let mut read = ChannelRead {
            handle,
            data: data.as_mut_ptr(),
            data_len: data.capacity(),
            handles: handles.as_mut_ptr().cast(),
            handles_len: handles.capacity(),
        };
        let res = channel_read(&mut read);
        match res {
            ChannelReadResult::Ok => unsafe {
                data.set_len(read.data_len);
                handles.set_len(read.handles_len);
                return res;
            },
            ChannelReadResult::Empty => {
                object_wait(handle, ObjectSignal::READABLE);
            }
            ChannelReadResult::Size => {
                if read.data_len > data.len() {
                    data.reserve(read.data_len - data.len());
                }
                if read.handles_len > handles.len() {
                    handles.reserve(read.handles_len - handles.len());
                }
            }
            _ => unsafe {
                data.set_len(0);
                handles.set_len(0);
                return res;
            },
        }
    }
}

pub fn channel_read_val<V>(
    handle: KernelReferenceID,
    data: &mut MaybeUninit<V>,
    handles: &mut Vec<KernelReferenceID>,
) -> ChannelReadResult {
    let mut read = ChannelRead {
        handle,
        data: data.as_mut_ptr().cast(),
        data_len: size_of::<V>(),
        handles: handles.as_mut_ptr().cast(),
        handles_len: handles.capacity(),
    };

    loop {
        let res = channel_read(&mut read);
        match res {
            ChannelReadResult::Ok if read.data_len == size_of::<V>() => unsafe {
                handles.set_len(read.handles_len);
                return res;
            },
            ChannelReadResult::Ok => unsafe {
                handles.set_len(read.handles_len);
                while let Some(h) = handles.pop() {
                    delete_reference(h);
                }
                return ChannelReadResult::Size;
            },
            ChannelReadResult::Empty => {
                object_wait(handle, ObjectSignal::READABLE);
            }
            _ => unsafe {
                handles.set_len(0);
                return res;
            },
        }
    }
}
