use bytecheck::CheckBytes;
use kernel_sys::{
    syscall::{
        sys_interrupt_acknowledge, sys_interrupt_create, sys_interrupt_set_port,
        sys_interrupt_trigger, sys_interrupt_wait,
    },
    types::SyscallResult,
};
use rkyv::{
    Archive, Deserialize, Portable, Serialize,
    rancor::{Error, Source},
};

use crate::{handle::Handle, ipc::IPCChannel, port::Port};

#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct Interrupt(Handle);

impl Interrupt {
    pub const fn from_handle(handle: Handle) -> Self {
        Self(handle)
    }

    pub const fn handle(&self) -> &Handle {
        &self.0
    }

    pub fn into_inner(self) -> Handle {
        let Self(handle) = self;
        handle
    }

    pub fn new() -> Interrupt {
        unsafe { Self::from_handle(Handle::from_id(sys_interrupt_create())) }
    }

    pub fn wait(&self) -> SyscallResult {
        sys_interrupt_wait(*self.0)
    }

    pub fn trigger(&self) -> SyscallResult {
        sys_interrupt_trigger(*self.0)
    }

    pub fn acknowledge(&self) -> SyscallResult {
        sys_interrupt_acknowledge(*self.0)
    }

    pub fn set_port(&self, port: &Port, key: u64) -> SyscallResult {
        sys_interrupt_set_port(*self.0, **port.handle(), key)
    }
}

#[derive(Debug, Clone, Copy, Archive, Serialize, Deserialize, Portable, CheckBytes)]
#[repr(u8)]
pub enum InterruptVector {
    Keyboard,
    Mouse,
    PCI,
    COM1,
}

pub struct InterruptsService(IPCChannel);

impl InterruptsService {
    pub fn from_channel(channel: IPCChannel) -> Self {
        Self(channel)
    }

    pub fn get_interrupt(&mut self, vector: InterruptVector) -> Option<Interrupt> {
        self.0.send(&vector).assert_ok();
        self.0.recv().unwrap().deserialize().unwrap()
    }
}

pub struct InterruptsServiceExecutor<I: InterruptsServiceImpl> {
    channel: IPCChannel,
    service: I,
}

pub trait InterruptsServiceImpl {
    fn get_interrupt(&mut self, vector: InterruptVector) -> Option<Interrupt>;
}

impl<I: InterruptsServiceImpl> InterruptsServiceExecutor<I> {
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

            let (vector, _) = msg.access::<InterruptVector>()?;

            let res = self.service.get_interrupt(*vector);
            self.channel.send(&res).into_err().map_err(Error::new)?;
        }
    }
}
