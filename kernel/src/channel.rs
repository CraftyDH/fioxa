use alloc::{
    boxed::Box,
    collections::vec_deque::VecDeque,
    sync::{Arc, Weak},
};
use kernel_sys::types::{ObjectSignal, SyscallError, SyscallResult};

use crate::{
    mutex::Spinlock,
    object::{KObject, KObjectSignal},
    scheduling::process::KernelValue,
};

pub const CHANNEL_CAPACITY: usize = 10000;

pub struct KChannelInner {
    signal: KObjectSignal,
    open: bool,
    queue: VecDeque<ChannelMessage>,
}

impl Default for KChannelInner {
    fn default() -> Self {
        Self {
            signal: Default::default(),
            open: true,
            queue: Default::default(),
        }
    }
}

pub struct KChannelHandle {
    channel: Spinlock<KChannelInner>,
    peer: Weak<KChannelHandle>,
}

impl KObject for KChannelHandle {
    fn signals<T>(&self, f: impl FnOnce(&mut KObjectSignal) -> T) -> T {
        let mut chan = self.channel.lock();
        f(&mut chan.signal)
    }
}

impl KChannelHandle {
    pub fn close(&self) {
        // close this handle
        let mut chan = self.channel.lock();
        chan.open = false;
        chan.signal.set_signal(ObjectSignal::CHANNEL_CLOSED, true);
        chan.queue.clear();
        drop(chan);

        // notify peer that we are closed
        if let Some(p) = self.peer.upgrade() {
            let mut chan = p.channel.lock();
            chan.open = false;
            chan.signal.set_signal(ObjectSignal::CHANNEL_CLOSED, true);
        };
    }

    pub fn send(&self, msg: ChannelMessage) -> SyscallResult {
        let Some(peer) = self.peer.upgrade() else {
            return Err(SyscallError::ChannelClosed);
        };

        let mut chan = peer.channel.lock();

        if !chan.open {
            return Err(SyscallError::ChannelClosed);
        }

        if chan.queue.len() > CHANNEL_CAPACITY {
            return Err(SyscallError::ChannelFull);
        }

        chan.queue.push_back(msg);
        chan.signal.set_signal(ObjectSignal::READABLE, true);

        Ok(())
    }

    pub fn read(&self, max_bytes: usize, max_handles: usize) -> Result<ChannelMessage, ReadError> {
        let mut chan = self.channel.lock();

        // even if it's closed we want to allow it to be drained
        let packet = chan.queue.pop_front().ok_or_else(|| {
            if chan.open {
                ReadError::Empty
            } else {
                ReadError::Closed
            }
        })?;

        let handles_len = packet.handles.as_ref().map(|h| h.len()).unwrap_or(0);
        if packet.data.len() > max_bytes || handles_len > max_handles {
            let err = ReadError::Size {
                min_bytes: packet.data.len(),
                min_handles: handles_len,
            };
            chan.queue.push_front(packet);
            return Err(err);
        }

        let empty = chan.queue.is_empty();
        chan.signal.set_signal(ObjectSignal::READABLE, !empty);

        Ok(packet)
    }
}

pub enum ReadError {
    Empty,
    Closed,
    Size {
        min_bytes: usize,
        min_handles: usize,
    },
}

impl Drop for KChannelHandle {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct ChannelMessage {
    pub data: Box<[u8]>,
    pub handles: Option<Box<[KernelValue]>>,
}

pub fn channel_create() -> (Arc<KChannelHandle>, Arc<KChannelHandle>) {
    let mut right = None;
    let left = Arc::new_cyclic(|left| {
        let r = Arc::new(KChannelHandle {
            channel: Default::default(),
            peer: left.clone(),
        });
        let peer = Arc::downgrade(&r);
        right = Some(r);
        KChannelHandle {
            channel: Default::default(),
            peer,
        }
    });
    (left, right.unwrap())
}
