use core::num::NonZeroUsize;

use alloc::vec::Vec;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::ToPrimitive;

use crate::{make_syscall, syscall::MESSAGE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub NonZeroUsize);

/// This is a kernel ref counted immutable object
#[derive(Debug, PartialEq, Eq)]
pub struct MessageHandle(MessageId);

#[derive(FromPrimitive, ToPrimitive)]
pub enum SyscallMessageAction {
    Create = 0,
    GetSize,
    Read,
    Clone,
    Drop,
}

#[repr(C)]
pub union MessageCreate {
    pub before: (*const u8, usize),
    pub after: MessageId,
}

#[repr(C)]
pub union MessageGetSize {
    pub before: MessageId,
    pub after: usize,
}

#[repr(C)]
pub struct MessageRead {
    pub id: MessageId,
    pub ptr: (*mut u8, usize),
}

#[repr(C)]
pub struct MessageClone(pub MessageId);

#[repr(C)]
pub struct MessageDrop(pub MessageId);

impl MessageHandle {
    pub const unsafe fn new_unchecked(id: MessageId) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> MessageId {
        self.0
    }

    unsafe fn make_syscall<T>(action: SyscallMessageAction, arg: &mut T) {
        let action = ToPrimitive::to_usize(&action).unwrap();
        make_syscall!(MESSAGE, action, arg as *mut T);
    }

    pub fn create(buf: &[u8]) -> Self {
        unsafe {
            let mut msg = MessageCreate {
                before: (buf.as_ptr(), buf.len()),
            };

            Self::make_syscall(SyscallMessageAction::Create, &mut msg);
            Self(msg.after)
        }
    }

    pub fn get_size(&self) -> usize {
        unsafe {
            let mut msg = MessageGetSize { before: self.0 };

            Self::make_syscall(SyscallMessageAction::GetSize, &mut msg);
            msg.after
        }
    }

    pub fn read(&self, buffer: &mut [u8]) {
        unsafe {
            let mut msg = MessageRead {
                id: self.0,
                ptr: (buffer.as_mut_ptr(), buffer.len()),
            };

            Self::make_syscall(SyscallMessageAction::Read, &mut msg);
        }
    }

    pub fn read_vec(&self) -> Vec<u8> {
        let size = self.get_size();
        let mut vec = vec![0; size];
        self.read(&mut vec);
        vec
    }
}

impl Clone for MessageHandle {
    fn clone(&self) -> Self {
        unsafe {
            let mut msg = MessageClone(self.0);
            Self::make_syscall(SyscallMessageAction::Clone, &mut msg);
            Self(self.0)
        }
    }
}
