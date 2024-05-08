use core::num::NonZeroUsize;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use serde::{Deserialize, Serialize};

use crate::make_syscall;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KernelReferenceID(pub NonZeroUsize);

impl KernelReferenceID {
    pub const fn from_usize(val: usize) -> Option<KernelReferenceID> {
        match NonZeroUsize::new(val) {
            Some(v) => Some(KernelReferenceID(v)),
            None => None,
        }
    }
}

pub const REFERENCE_STDOUT: KernelReferenceID =
    unsafe { KernelReferenceID(NonZeroUsize::new_unchecked(1)) };

#[derive(FromPrimitive, ToPrimitive)]
pub enum ReferenceOperation {
    Clone,
    Delete,
    GetType,
}

#[derive(Debug, FromPrimitive, ToPrimitive, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    None,
    Event,
    EventQueue,
    Socket,
    SocketListener,
    Message,
    Process,
}

#[derive(Debug, PartialEq, Eq)]
pub struct KernelReference(KernelReferenceID);

impl KernelReference {
    pub const fn from_id(id: KernelReferenceID) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> KernelReferenceID {
        self.0
    }
}

impl Drop for KernelReference {
    fn drop(&mut self) {
        delete_reference(self.id())
    }
}

impl Clone for KernelReference {
    fn clone(&self) -> Self {
        Self(clone_reference(self.id()))
    }
}

pub fn clone_reference(kref: KernelReferenceID) -> KernelReferenceID {
    unsafe {
        let id: usize;
        make_syscall!(crate::syscall::OBJECT, ReferenceOperation::Clone as usize, kref.0.get() => id);
        KernelReferenceID::from_usize(id).unwrap()
    }
}

pub fn delete_reference(kref: KernelReferenceID) {
    unsafe {
        make_syscall!(
            crate::syscall::OBJECT,
            ReferenceOperation::Delete as usize,
            kref.0.get()
        );
    }
}

pub fn get_type(kref: KernelReferenceID) -> KernelObjectType {
    unsafe {
        let id: usize;
        make_syscall!(crate::syscall::OBJECT, ReferenceOperation::GetType as usize, kref.0.get() => id);
        KernelObjectType::from_usize(id).unwrap()
    }
}
