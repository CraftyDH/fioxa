use kernel_sys::{
    syscall::{sys_object_wait, sys_process_exit_code},
    types::{ObjectSignal, SyscallError},
};
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::InlineAsBox,
};

use crate::{
    channel::Channel,
    handle::{FIRST_HANDLE, Handle},
    ipc::IPCChannel,
    service::Service,
};

pub static INIT_HANDLE_SERVICE: Service =
    unsafe { Service(Channel::from_handle(Handle::from_id(FIRST_HANDLE))) };

#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
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

    pub fn get_exit_code(&self) -> Result<usize, SyscallError> {
        sys_process_exit_code(*self.0)
    }

    pub fn blocking_exit_code(&mut self) -> usize {
        loop {
            match self.get_exit_code() {
                Ok(val) => return val,
                Err(SyscallError::ProcessStillRunning) => {
                    sys_object_wait(*self.0, ObjectSignal::PROCESS_EXITED).unwrap();
                }
                Err(e) => panic!("unknown err {e:?}"),
            };
        }
    }
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum InitHandleMessage<'a> {
    GetHandle(#[rkyv(with = InlineAsBox)] &'a str),
    PublishHandle(#[rkyv(with = InlineAsBox)] &'a str, Handle),
}

pub struct InitHandleService(IPCChannel);

impl InitHandleService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self(chan)
    }

    pub fn connect() -> Self {
        Self(IPCChannel::from_channel(
            INIT_HANDLE_SERVICE.connect().unwrap(),
        ))
    }

    pub fn get_handle(&mut self, name: &str) -> Option<Handle> {
        self.0.send(&InitHandleMessage::GetHandle(name)).unwrap();
        let mut msg = self.0.recv().unwrap();
        msg.deserialize().unwrap()
    }

    pub fn publish_handle(&mut self, name: &str, handle: Handle) -> bool {
        self.0
            .send(&InitHandleMessage::PublishHandle(name, handle))
            .unwrap();
        let mut msg = self.0.recv().unwrap();
        msg.deserialize().unwrap()
    }
}

pub struct InitHandleServiceExecutor<I: InitHandleServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: InitHandleServiceImpl> InitHandleServiceExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallError::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedInitHandleMessage>()?;

            match msg {
                ArchivedInitHandleMessage::GetHandle(name) => {
                    let res = self.service.get_handle(name);
                    self.channel.send(&res)
                }
                ArchivedInitHandleMessage::PublishHandle(name, handle) => {
                    let res = self.service.publish_handle(name, handle.deserialize(des)?);
                    self.channel.send(&res)
                }
            }
            .map_err(Error::new)?;
        }
    }
}

pub trait InitHandleServiceImpl {
    fn get_handle(&mut self, name: &str) -> Option<Handle>;

    fn publish_handle(&mut self, name: &str, handle: Handle) -> bool;
}
