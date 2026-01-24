use alloc::vec::Vec;
use kernel_sys::types::{KernelObjectType, ObjectSignal, SyscallError};
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
};

use crate::{backoff_sleep, channel::Channel, process::InitHandleService};

#[must_use]
pub struct ServiceExecutor<I: Fn(Channel)> {
    channel: Channel,
    service: I,
}

impl<I: Fn(Channel)> ServiceExecutor<I> {
    pub fn from_channel(channel: Channel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn with_name(name: &str, service: I) -> Self {
        let (l, r) = Channel::new();
        assert!(!InitHandleService::connect().publish_handle(name, r.into_inner()));

        Self {
            channel: l,
            service,
        }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            match run_service_iter(&self.channel, &self.service) {
                Ok(()) => (),
                Err(SyscallError::ChannelClosed) => return Ok(()),
                Err(SyscallError::ChannelEmpty) => {
                    self.channel.handle().wait(ObjectSignal::all()).unwrap();
                }
                Err(e) => return Err(Error::new(e)),
            }
        }
    }
}

pub fn run_service_iter(chan: &Channel, mut f: impl FnMut(Channel)) -> Result<(), SyscallError> {
    let mut vec = Vec::new();
    let handles = chan.read::<32>(&mut vec, false, false)?;

    for handle in handles {
        let ty = handle.get_type();
        if ty == KernelObjectType::Channel {
            f(Channel::from_handle(handle));
        } else {
            // TODO: Warn?
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct Service(pub Channel);

impl Service {
    pub fn connect(&self) -> Result<Channel, SyscallError> {
        let (left, right) = Channel::new();
        loop {
            match self.0.write(&[], &[**right.handle()]) {
                Ok(()) => return Ok(left),
                Err(SyscallError::ChannelFull) => {
                    self.0.handle().wait(ObjectSignal::all()).unwrap()
                }
                Err(e) => return Err(e),
            };
        }
    }

    pub fn try_get_by_name(name: &str) -> Option<Self> {
        let mut serv = InitHandleService::connect();
        let chan = Channel::from_handle(serv.get_handle(name)?);
        Some(Self(chan))
    }

    pub fn get_by_name(name: &str) -> Self {
        let mut serv = InitHandleService::connect();
        let chan = Channel::from_handle(backoff_sleep(|| serv.get_handle(name)));
        Self(chan)
    }
}
