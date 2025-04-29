use kernel_sys::types::SyscallResult;
use rkyv::rancor::{Error, Source};

use crate::{channel::Channel, ipc::IPCChannel, process::INIT_HANDLE_SERVICE};

pub struct Service(IPCChannel);

impl Service {
    pub fn from_channel(channel: IPCChannel) -> Self {
        Self(channel)
    }

    pub fn send_consumer(&mut self, channel: Channel) {
        self.0.send(&channel).assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }
}

#[must_use]
pub struct ServiceExecutor<I: Fn(Channel)> {
    channel: IPCChannel,
    service: I,
}

impl<I: Fn(Channel)> ServiceExecutor<I> {
    pub fn from_channel(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn with_name(name: &str, service: I) -> Self {
        let (l, r) = Channel::new();
        assert!(!INIT_HANDLE_SERVICE.lock().publish_service(name, r));
        Self {
            channel: IPCChannel::from_channel(l),
            service,
        }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };

            self.service.call(msg.deserialize()?);

            self.channel.send(&()).into_err().map_err(Error::new)?;
        }
    }
}
