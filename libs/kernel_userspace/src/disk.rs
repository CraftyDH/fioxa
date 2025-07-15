use alloc::vec::Vec;
use kernel_sys::types::SyscallResult;
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
};

use crate::{
    channel::Channel,
    disk::ata::ATADiskIdentify,
    ipc::{IPCChannel, IPCIterator, TypedIPCMessage},
};

pub mod ata;

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum DiskDeviceRequest {
    Read { sector: u64, count: u64 },
    Identify,
}

pub struct DiskService {
    chan: IPCChannel,
}

impl DiskService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self { chan }
    }

    pub fn identify(&mut self) -> ATADiskIdentify {
        self.chan.send(&DiskDeviceRequest::Identify).assert_ok();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn read(&mut self, sector: u64, count: u64) -> TypedIPCMessage<'_, Vec<u8>> {
        self.chan
            .send(&DiskDeviceRequest::Read { sector, count })
            .assert_ok();
        TypedIPCMessage::new(self.chan.recv().unwrap())
    }
}

pub struct DiskServiceExecutor<I: DiskServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: DiskServiceImpl> DiskServiceExecutor<I> {
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
            let (msg, _) = msg.access::<ArchivedDiskDeviceRequest>()?;

            let err = match msg {
                ArchivedDiskDeviceRequest::Read { sector, count } => {
                    let res = self.service.read(sector.to_native(), count.to_native());
                    self.channel.send(&res)
                }
                ArchivedDiskDeviceRequest::Identify => {
                    let res = self.service.identify();
                    self.channel.send(&res)
                }
            };
            err.into_err().map_err(Error::new)?;
        }
    }
}

pub trait DiskServiceImpl {
    fn read(&mut self, sector: u64, length: u64) -> Vec<u8>;
    fn identify(&mut self) -> ATADiskIdentify;
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum DiskControllerMessage {
    RegisterDisk(Channel),
    GetDisks { updates: bool },
}

pub struct DiskControllerService {
    chan: IPCChannel,
}

impl DiskControllerService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self { chan }
    }

    pub fn register_disk(&mut self, chan: Channel) {
        self.chan
            .send(&DiskControllerMessage::RegisterDisk(chan))
            .assert_ok();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn get_disks(&mut self, updates: bool) -> DiskIterator {
        self.chan
            .send(&DiskControllerMessage::GetDisks { updates })
            .assert_ok();
        let chan: Channel = self.chan.recv().unwrap().deserialize().unwrap();
        DiskIterator(IPCIterator::from(IPCChannel::from_channel(chan)))
    }
}

pub struct DiskIterator(IPCIterator<Channel>);

impl Iterator for DiskIterator {
    type Item = DiskService;

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .next()
            .map(|e| DiskService::from_channel(IPCChannel::from_channel(e)))
    }
}

pub struct DiskControllerExecutor<I: DiskControllerImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: DiskControllerImpl> DiskControllerExecutor<I> {
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
            let (msg, des) = msg.access::<ArchivedDiskControllerMessage>()?;

            let err = match msg {
                ArchivedDiskControllerMessage::RegisterDisk(disk) => {
                    self.service.register_disk(disk.deserialize(des).unwrap());
                    self.channel.send(&())
                }
                ArchivedDiskControllerMessage::GetDisks { updates } => {
                    let res = self.service.get_disks(*updates);
                    self.channel.send(&res)
                }
            };
            err.into_err().map_err(Error::new)?;
        }
    }
}

pub trait DiskControllerImpl {
    fn register_disk(&mut self, chan: Channel);
    fn get_disks(&mut self, updates: bool) -> Channel;
}
