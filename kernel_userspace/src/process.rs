use core::u64;

use alloc::vec::Vec;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use serde::{Deserialize, Serialize};

use crate::{
    channel::{channel_read_rs, channel_write_rs},
    make_syscall,
    object::{object_wait, KernelReference, KernelReferenceID, ObjectSignal, REFERENCE_FIRST},
    service::serialize,
};

#[derive(FromPrimitive, ToPrimitive)]
pub enum KernelProcessOperation {
    GetExitCode,
    Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, ToPrimitive)]
pub enum ProcessExit {
    Exited,
    NotExitedYet,
}

pub fn process_get_exit_code(handle: KernelReferenceID) -> ProcessExit {
    let res: usize;
    unsafe {
        make_syscall!(
            crate::syscall::PROCESS,
            KernelProcessOperation::GetExitCode as usize,
            handle.0.get() => res
        );
        ProcessExit::from_usize(res).unwrap()
    }
}

pub fn process_kill(handle: KernelReferenceID) {
    unsafe {
        make_syscall!(
            crate::syscall::PROCESS,
            KernelProcessOperation::Kill as usize,
            handle.0.get()
        );
    }
}

pub struct ProcessHandle {
    handle: KernelReference,
}

impl ProcessHandle {
    pub fn from_kref(kref: KernelReference) -> Self {
        Self { handle: kref }
    }

    pub fn get_exit_code(&self) -> ProcessExit {
        process_get_exit_code(self.handle.id())
    }

    pub fn blocking_exit_code(&mut self) -> ProcessExit {
        loop {
            match process_get_exit_code(self.handle.id()) {
                ProcessExit::NotExitedYet => (),
                a => return a,
            };
            object_wait(self.handle.id(), ObjectSignal::PROCESS_EXITED);
        }
    }

    pub fn kill(&self) {
        process_kill(self.handle.id())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InitHandleMessage<'a> {
    GetHandle(&'a str),
    PublishHandle(&'a str),
    Clone,
}

pub fn get_handle(name: &str) -> Option<KernelReferenceID> {
    let mut buf = Vec::new();
    let data = serialize(&InitHandleMessage::GetHandle(name), &mut buf);
    assert!(channel_write_rs(REFERENCE_FIRST, data, &[]));

    let mut handles = Vec::with_capacity(1);

    match channel_read_rs(REFERENCE_FIRST, &mut buf, &mut handles) {
        crate::channel::ChannelReadResult::Ok => (),
        e => panic!("error {e:?}"),
    }

    if buf[0] == 1 {
        Some(handles[0])
    } else {
        None
    }
}

pub fn publish_handle(name: &str, handle: KernelReferenceID) -> bool {
    let mut buf = Vec::new();
    let data = serialize(&InitHandleMessage::PublishHandle(name), &mut buf);
    assert!(channel_write_rs(REFERENCE_FIRST, data, &[handle]));

    let mut handles = Vec::with_capacity(1);

    match channel_read_rs(REFERENCE_FIRST, &mut buf, &mut handles) {
        crate::channel::ChannelReadResult::Ok => (),
        e => panic!("error {e:?}"),
    }

    buf[0] == 1
}

pub fn clone_init_service() -> KernelReferenceID {
    let mut buf = Vec::new();
    let data = serialize(&InitHandleMessage::Clone, &mut buf);
    assert!(channel_write_rs(REFERENCE_FIRST, data, &[]));

    let mut handles = Vec::with_capacity(1);

    match channel_read_rs(REFERENCE_FIRST, &mut buf, &mut handles) {
        crate::channel::ChannelReadResult::Ok => (),
        e => panic!("error {e:?}"),
    }

    handles[0]
}
