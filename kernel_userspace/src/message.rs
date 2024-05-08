use alloc::vec::Vec;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::ToPrimitive;

use crate::{
    make_syscall,
    object::{KernelReference, KernelReferenceID},
    syscall::MESSAGE,
};

/// This is a kernel ref counted immutable object
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHandle(KernelReference);

#[derive(FromPrimitive, ToPrimitive)]
pub enum SyscallMessageAction {
    Create = 0,
    GetSize,
    Read,
}

#[repr(C)]
pub union MessageCreate {
    pub before: (*const u8, usize),
    pub after: KernelReferenceID,
}

#[repr(C)]
pub union MessageGetSize {
    pub before: KernelReferenceID,
    pub after: usize,
}

#[repr(C)]
pub struct MessageRead {
    pub id: KernelReferenceID,
    pub ptr: (*mut u8, usize),
}

impl MessageHandle {
    pub const fn from_kref(id: KernelReference) -> Self {
        Self(id)
    }

    pub const fn kref(&self) -> &KernelReference {
        &self.0
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
            Self(KernelReference::from_id(msg.after))
        }
    }

    pub fn get_size(&self) -> usize {
        unsafe {
            let mut msg = MessageGetSize {
                before: self.0.id(),
            };

            Self::make_syscall(SyscallMessageAction::GetSize, &mut msg);
            msg.after
        }
    }

    pub fn read(&self, buffer: &mut [u8]) {
        unsafe {
            let mut msg = MessageRead {
                id: self.0.id(),
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

    pub fn read_into_vec(&self, vec: &mut Vec<u8>) {
        let size = self.get_size();
        vec.resize(size, 0);
        self.read(vec);
    }
}
