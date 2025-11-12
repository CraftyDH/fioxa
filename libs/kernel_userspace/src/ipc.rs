use core::{
    fmt::{Debug, Display},
    marker::PhantomData,
    ops::Deref,
};

use alloc::{
    borrow::{Cow, ToOwned},
    vec::Vec,
};
use bytecheck::CheckBytes;
use kernel_sys::types::{Hid, SyscallError};
use rkyv::{
    Archive, Deserialize, Portable, Serialize, SerializeUnsized,
    api::high::HighValidator,
    rancor::{Error, Fallible, Source, Strategy},
    ser::{Allocator, Positional, Writer, allocator::Arena},
    traits::NoUndef,
    with::{AsOwned, InlineAsBox},
};

use crate::{backoff_sleep, channel::Channel, handle::Handle, process::INIT_HANDLE_SERVICE};

#[derive(Archive, Serialize, Deserialize)]
pub struct IPCBox<'a, T: ?Sized + 'a>(#[rkyv(with = InlineAsBox)] pub &'a T);

#[derive(Archive, Serialize, Deserialize)]
pub struct CowAsOwned<'a, T: ToOwned + ?Sized + 'a>(#[rkyv(with = AsOwned)] pub Cow<'a, T>);

impl<'a, T: ToOwned + Debug + ?Sized + 'a> Debug for CowAsOwned<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("CowAsOwned").field(&&*self.0).finish()
    }
}

impl<'a, T: ToOwned + ?Sized + 'a> Clone for CowAsOwned<'a, T>
where
    T::Owned: Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<'a, T: ToOwned + ?Sized + 'a> From<&'a T> for CowAsOwned<'a, T> {
    fn from(value: &'a T) -> Self {
        Self(Cow::Borrowed(value))
    }
}

impl<'a, T: ToOwned + ?Sized + 'a> Deref for CowAsOwned<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct IPCChannel {
    channel: Channel,
    buffer: Vec<u8>,
    handles: heapless::Vec<Hid, 32>,
    arena: Arena,
}

impl IPCChannel {
    pub fn from_channel(channel: Channel) -> Self {
        Self {
            channel,
            buffer: Vec::new(),
            handles: heapless::Vec::new(),
            arena: Arena::new(),
        }
    }

    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    pub fn connect(name: &str) -> Self {
        Self::from_channel(backoff_sleep(|| {
            INIT_HANDLE_SERVICE.lock().get_service(name)
        }))
    }

    pub fn send<'a>(
        &'a mut self,
        val: &impl SerializeUnsized<Strategy<SerializeMessage<'a>, Error>>,
    ) -> Result<(), SyscallError> {
        self.buffer.clear();
        self.handles.clear();

        let mut ser = SerializeMessage {
            buffer: &mut self.buffer,
            handles: &mut self.handles,
            arena: &mut self.arena,
        };

        rkyv::api::serialize_using(val, &mut ser).unwrap();

        self.channel.write(ser.buffer, ser.handles)
    }

    pub fn recv<'a>(&'a mut self) -> Result<IPCMessage<'a>, SyscallError> {
        let handles = self.channel.read::<32>(&mut self.buffer, true, true)?;
        let handles = handles.into_iter().map(Some).collect();
        Ok(IPCMessage {
            buffer: &self.buffer,
            deserializer: DeserializeMessage { handles },
        })
    }
}

pub struct SerializeMessage<'a> {
    buffer: &'a mut Vec<u8>,
    handles: &'a mut heapless::Vec<Hid, 32>,
    arena: &'a mut Arena,
}

impl<E> Writer<E> for SerializeMessage<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), E> {
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }
}

impl Fallible for SerializeMessage<'_> {
    type Error = Error;
}

unsafe impl Allocator for SerializeMessage<'_> {
    unsafe fn push_alloc(
        &mut self,
        layout: core::alloc::Layout,
    ) -> Result<core::ptr::NonNull<[u8]>, <Self as rkyv::rancor::Fallible>::Error> {
        unsafe { self.arena.acquire().push_alloc(layout) }
    }

    unsafe fn pop_alloc(
        &mut self,
        ptr: core::ptr::NonNull<u8>,
        layout: core::alloc::Layout,
    ) -> Result<(), <Self as rkyv::rancor::Fallible>::Error> {
        unsafe { self.arena.acquire().pop_alloc(ptr, layout) }
    }
}

impl Positional for SerializeMessage<'_> {
    fn pos(&self) -> usize {
        self.buffer.len()
    }
}

pub struct IPCMessage<'a> {
    buffer: &'a [u8],
    deserializer: DeserializeMessage,
}

pub struct DeserializeMessage {
    handles: heapless::Vec<Option<Handle>, 32>,
}

impl<'s> IPCMessage<'s> {
    pub fn access<T: Portable + for<'a> CheckBytes<HighValidator<'a, Error>>>(
        &mut self,
    ) -> Result<(&T, &mut Strategy<DeserializeMessage, Error>), Error> {
        rkyv::access(self.buffer).map(|l| (l, Strategy::wrap(&mut self.deserializer)))
    }

    pub fn deserialize<T: Archive>(&'s mut self) -> Result<T, Error>
    where
        T::Archived: Portable
            + for<'a> CheckBytes<HighValidator<'a, Error>>
            + Deserialize<T, Strategy<DeserializeMessage, Error>>,
    {
        let access = rkyv::access::<T::Archived, Error>(self.buffer)?;
        access.deserialize(Strategy::wrap(&mut self.deserializer))
    }
}

pub struct TypedIPCMessage<'a, T> {
    message: IPCMessage<'a>,
    _t: PhantomData<T>,
}

impl<'s, T> TypedIPCMessage<'s, T> {
    pub fn new(message: IPCMessage<'s>) -> Self {
        Self {
            message,
            _t: PhantomData,
        }
    }
}

impl<'s, T: Portable + for<'a> CheckBytes<HighValidator<'a, Error>>> TypedIPCMessage<'s, T> {
    pub fn access(&mut self) -> Result<(&T, &mut Strategy<DeserializeMessage, Error>), Error> {
        self.message.access()
    }
}

impl<'s, T: Archive> TypedIPCMessage<'s, T> {
    pub fn deserialize(&'s mut self) -> Result<T, Error>
    where
        T::Archived: Portable
            + for<'a> CheckBytes<HighValidator<'a, Error>>
            + Deserialize<T, Strategy<DeserializeMessage, Error>>,
    {
        self.message.deserialize()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TooManyHandles;

impl Display for TooManyHandles {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Too many handles")
    }
}

impl core::error::Error for TooManyHandles {}

impl Serialize<Strategy<SerializeMessage<'_>, Error>> for Handle {
    fn serialize(
        &self,
        serializer: &mut Strategy<SerializeMessage<'_>, Error>,
    ) -> Result<Self::Resolver, Error> {
        let idx: u8 = serializer.handles.len().try_into().map_err(Error::new)?;
        serializer
            .handles
            .push(**self)
            .map_err(|_| Error::new(TooManyHandles))?;
        Ok(HandleResolver(idx))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HandleNotFound(bool);

impl Display for HandleNotFound {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 {
            f.write_str("Handle not found (not passed)")
        } else {
            f.write_str("Handle not found (already used)")
        }
    }
}

impl core::error::Error for HandleNotFound {}

impl Deserialize<Handle, Strategy<DeserializeMessage, Error>> for ArchivedHandle {
    fn deserialize(
        &self,
        deserializer: &mut Strategy<DeserializeMessage, Error>,
    ) -> Result<Handle, Error> {
        deserializer
            .handles
            .get_mut(self.0 as usize)
            .map_or_else(
                || Err(HandleNotFound(false)),
                |h| h.take().ok_or(HandleNotFound(true)),
            )
            .map_err(Error::new)
    }
}

#[derive(Debug, Portable, CheckBytes)]
#[repr(transparent)]
pub struct ArchivedHandle(u8);

unsafe impl NoUndef for ArchivedHandle {}

impl Archive for Handle {
    type Archived = ArchivedHandle;

    type Resolver = HandleResolver;

    fn resolve(&self, resolver: Self::Resolver, out: rkyv::Place<Self::Archived>) {
        out.write(ArchivedHandle(resolver.0));
    }
}

pub struct HandleResolver(u8);

pub struct IPCIterator<T> {
    channel: IPCChannel,
    _ty: PhantomData<T>,
}

impl<T> Iterator for IPCIterator<T>
where
    T: Archive,
    <T as Archive>::Archived: Portable
        + for<'a> CheckBytes<HighValidator<'a, Error>>
        + Deserialize<T, Strategy<DeserializeMessage, Error>>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self.channel.recv() {
            Ok(mut val) => Some(val.deserialize().unwrap()),
            Err(SyscallError::ChannelClosed) => None,
            Err(e) => panic!("failed to get next got: {e}"),
        }
    }
}

impl<T> From<IPCChannel> for IPCIterator<T> {
    fn from(channel: IPCChannel) -> Self {
        Self {
            channel,
            _ty: PhantomData,
        }
    }
}
