use conquer_once::spin::Lazy;
use kernel_sys::{
    syscall::{sys_object_wait, sys_process_exit_code},
    types::{ObjectSignal, SyscallResult},
};
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::InlineAsBox,
};
use spin::Mutex;

use crate::{
    channel::Channel,
    handle::{FIRST_HANDLE, Handle},
    ipc::IPCChannel,
};

pub static INIT_HANDLE_SERVICE: Lazy<Mutex<InitHandleService>> = Lazy::new(|| {
    Mutex::new(InitHandleService::from_channel(IPCChannel::from_channel(
        Channel::from_handle(FIRST_HANDLE),
    )))
});

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

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum InitHandleMessage<'a> {
    GetService(#[rkyv(with = InlineAsBox)] &'a str),
    PublishService(#[rkyv(with = InlineAsBox)] &'a str, Channel),
    Clone,
}

pub struct InitHandleService(IPCChannel);

impl InitHandleService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self(chan)
    }

    pub fn get_service(&mut self, name: &str) -> Option<Channel> {
        self.0
            .send(&InitHandleMessage::GetService(name))
            .assert_ok();
        let mut msg = self.0.recv().unwrap();
        msg.deserialize().unwrap()
    }

    pub fn publish_service(&mut self, name: &str, handle: Channel) -> bool {
        self.0
            .send(&InitHandleMessage::PublishService(name, handle))
            .assert_ok();
        let mut msg = self.0.recv().unwrap();
        msg.deserialize().unwrap()
    }

    pub fn clone_init_service(&mut self) -> Channel {
        self.0.send(&InitHandleMessage::Clone).assert_ok();
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
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedInitHandleMessage>()?;

            match msg {
                ArchivedInitHandleMessage::GetService(name) => {
                    let res = self.service.get_service(name);
                    self.channel.send(&res)
                }
                ArchivedInitHandleMessage::PublishService(name, handle) => {
                    let res = self.service.publish_service(name, handle.deserialize(des)?);
                    self.channel.send(&res)
                }
                ArchivedInitHandleMessage::Clone => {
                    self.channel.send(&self.service.clone_init_service())
                }
            }
            .into_err()
            .map_err(Error::new)?;
        }
    }
}

pub trait InitHandleServiceImpl {
    fn get_service(&mut self, name: &str) -> Option<Channel>;

    fn publish_service(&mut self, name: &str, handle: Channel) -> bool;

    fn clone_init_service(&mut self) -> Channel;
}
