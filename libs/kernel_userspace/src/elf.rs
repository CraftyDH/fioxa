use kernel_sys::types::SyscallError;
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::InlineAsBox,
};
use thiserror::Error;

use crate::{
    handle::Handle,
    ipc::{CowAsOwned, IPCChannel},
    message::MessageHandle,
    process::ProcessHandle,
};

#[derive(Debug, Clone, Error, Archive, Serialize, Deserialize)]
pub enum LoadElfError {
    #[error("internal error")]
    InternalError,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct SpawnElfProcess<'a> {
    pub elf: CowAsOwned<'a, MessageHandle>,
    #[rkyv(with = InlineAsBox)]
    pub args: &'a [u8],
    #[rkyv(with = InlineAsBox)]
    pub initial_refs: &'a [CowAsOwned<'a, Handle>],
}

pub struct ElfLoaderService(IPCChannel);

impl ElfLoaderService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self(chan)
    }

    pub fn spawn(
        &mut self,
        elf: &MessageHandle,
        args: &[u8],
        initial_refs: &[&Handle],
    ) -> Result<ProcessHandle, LoadElfError> {
        let initial_refs: heapless::Vec<_, 31> = initial_refs.iter().map(|e| (*e).into()).collect();

        let spawn = SpawnElfProcess {
            elf: elf.into(),
            args,
            initial_refs: &initial_refs,
        };

        self.0.send(&spawn).unwrap();
        let mut res = self.0.recv().unwrap();

        res.deserialize().unwrap()
    }
}

pub struct ElfLoaderServiceExecutor<I: ElfLoaderServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: ElfLoaderServiceImpl> ElfLoaderServiceExecutor<I> {
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
            let (spawn, des) = msg.access::<ArchivedSpawnElfProcess>()?;
            let elf = spawn.elf.0.deserialize(des)?;

            let hids: heapless::Vec<Handle, 31> = spawn
                .initial_refs
                .iter()
                .flat_map(|h| h.0.deserialize(des))
                .collect();

            let res = self.service.spawn(elf, &spawn.args, &hids);
            self.channel.send(&res).map_err(Error::new)?;
        }
    }
}

pub trait ElfLoaderServiceImpl {
    fn spawn(
        &mut self,
        elf: MessageHandle,
        args: &[u8],
        initial_refs: &[Handle],
    ) -> Result<ProcessHandle, LoadElfError>;
}
