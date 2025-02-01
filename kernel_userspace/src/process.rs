use core::u64;

use alloc::vec::Vec;
use kernel_sys::{
    syscall::{sys_object_wait, sys_process_exit_code},
    types::{Hid, ObjectSignal, SyscallResult},
};
use serde::{Deserialize, Serialize};

use crate::{
    channel::{Channel, FIRST_HANDLE_CHANNEL},
    handle::Handle,
    service::serialize,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessHandle(Handle);

impl ProcessHandle {
    pub fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }

    pub const fn handle(&self) -> &Handle {
        &self.0
    }

    pub fn into_inner(self) -> Handle {
        let Self(handle) = self;
        handle
    }

    pub fn get_exit_code(&self) -> Result<usize, SyscallResult> {
        sys_process_exit_code(*self.0)
    }

    pub fn blocking_exit_code(&mut self) -> usize {
        loop {
            match self.get_exit_code() {
                Ok(val) => return val,
                Err(SyscallResult::ProcessStillRunning) => {
                    sys_object_wait(*self.0, ObjectSignal::PROCESS_EXITED).unwrap();
                }
                Err(e) => panic!("unknown err {e:?}"),
            };
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InitHandleMessage<'a> {
    GetHandle(&'a str),
    PublishHandle(&'a str),
    Clone,
}

pub fn get_handle(name: &str) -> Option<Handle> {
    let mut buf = Vec::with_capacity(1);
    let data = serialize(&InitHandleMessage::GetHandle(name), &mut buf);

    FIRST_HANDLE_CHANNEL.write(&data, &[]).assert_ok();

    let mut handles = FIRST_HANDLE_CHANNEL
        .read::<1>(&mut buf, true, true)
        .unwrap();

    handles.pop()
}

pub fn publish_handle(name: &str, handle: Hid) -> bool {
    let mut buf = Vec::new();
    let data = serialize(&InitHandleMessage::PublishHandle(name), &mut buf);
    FIRST_HANDLE_CHANNEL.write(data, &[handle]).assert_ok();

    FIRST_HANDLE_CHANNEL
        .read::<0>(&mut buf, true, true)
        .unwrap();

    buf[0] == 1
}

pub fn clone_init_service() -> Channel {
    let mut buf = Vec::new();
    let data = serialize(&InitHandleMessage::Clone, &mut buf);
    FIRST_HANDLE_CHANNEL.write(data, &[]).assert_ok();

    let mut handles = FIRST_HANDLE_CHANNEL
        .read::<1>(&mut buf, true, true)
        .unwrap();

    // We know that we'll get a channel back
    Channel::from_handle(handles.pop().unwrap())
}
