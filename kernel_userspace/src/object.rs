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

pub const REFERENCE_FIRST: KernelReferenceID =
    unsafe { KernelReferenceID(NonZeroUsize::new_unchecked(1)) };

bitflags::bitflags! {
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct ObjectSignal: u64 {
        const READABLE = 1 << 1;

        const CHANNEL_CLOSED = 1 << 20;

        const PROCESS_EXITED = 1 << 20;
    }
}

#[derive(FromPrimitive, ToPrimitive)]
pub enum ReferenceOperation {
    Clone,
    Delete,
    GetType,
    Wait,
    WaitPort,
}

#[derive(Debug, FromPrimitive, ToPrimitive, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    None,
    Message,
    Process,
    Channel,
    Port,
    Interrupt,
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

/// Returns the current set whenever any bit from mask is set
pub fn object_wait(kref: KernelReferenceID, mask: ObjectSignal) -> ObjectSignal {
    unsafe {
        let val: u64;
        make_syscall!(crate::syscall::OBJECT, ReferenceOperation::Wait as usize, kref.0.get(), mask.bits() => val);
        ObjectSignal::from_bits_retain(val)
    }
}

#[repr(C)]
pub struct WaitPort {
    pub port_handle: KernelReferenceID,
    pub mask: u64,
    pub key: u64,
}

/// Returns the current set whenever any bit from mask is set
pub fn object_wait_port(kref: KernelReferenceID, port: &WaitPort) {
    unsafe {
        make_syscall!(
            crate::syscall::OBJECT,
            ReferenceOperation::WaitPort as usize,
            kref.0.get(),
            port
        );
    }
}

pub fn object_wait_port_rs(
    kref: KernelReferenceID,
    port: KernelReferenceID,
    mask: ObjectSignal,
    key: u64,
) {
    let wait = WaitPort {
        port_handle: port,
        mask: mask.bits(),
        key,
    };
    object_wait_port(kref, &wait);
}
