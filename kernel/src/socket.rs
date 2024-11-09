use alloc::{
    collections::VecDeque,
    string::String,
    sync::{Arc, Weak},
};
use conquer_once::spin::Lazy;
use hashbrown::HashMap;
use kernel_userspace::socket::SocketEvents;

use crate::{event::KEvent, mutex::Spinlock, scheduling::process::KernelValue};

pub static PUBLIC_SOCKETS: Lazy<Spinlock<HashMap<String, Arc<KSocketListener>>>> =
    Lazy::new(|| Spinlock::new(HashMap::new()));

pub struct KSocketListener {
    queue: Spinlock<VecDeque<Arc<KSocketHandle>>>,
    event: Arc<Spinlock<KEvent>>,
}

impl KSocketListener {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Default::default(),
            event: KEvent::new(),
        })
    }

    pub fn connect(&self) -> Arc<KSocketHandle> {
        let (left, right) = create_sockets(1000, 1000);
        let mut q = self.queue.lock();
        q.push_back(right);
        self.event.lock().set_level(true);
        drop(q);
        left
    }

    pub fn pop(&self) -> Option<Arc<KSocketHandle>> {
        let mut q = self.queue.lock();
        let v = q.pop_front();
        if q.is_empty() {
            self.event.lock().set_level(false);
        }
        drop(q);
        v
    }

    pub fn event(&self) -> Arc<Spinlock<KEvent>> {
        self.event.clone()
    }
}

pub struct KSocketHandle {
    other_side: Weak<KSocketHandle>,
    send_queue_full_event: Arc<Spinlock<KEvent>>,
    recv_queue_empty_event: Arc<Spinlock<KEvent>>,
    closed_event: Arc<Spinlock<KEvent>>,
    queue: Spinlock<VecDeque<KernelValue>>,
    /// soft limit of how many elements can be in the queue
    capacity: usize,
}

pub fn create_sockets(ltr_cap: usize, rtl_cap: usize) -> (Arc<KSocketHandle>, Arc<KSocketHandle>) {
    let mut left = None;
    let right = Arc::new_cyclic(|weak_right| {
        let left_arc = Arc::new(KSocketHandle {
            other_side: weak_right.clone(),
            send_queue_full_event: KEvent::new(),
            recv_queue_empty_event: KEvent::new(),
            closed_event: KEvent::new(),
            queue: Default::default(),
            capacity: rtl_cap,
        });
        let left_weak = Arc::downgrade(&left_arc);
        left = Some(left_arc);
        KSocketHandle {
            other_side: left_weak,
            send_queue_full_event: KEvent::new(),
            recv_queue_empty_event: KEvent::new(),
            closed_event: KEvent::new(),
            queue: Default::default(),
            capacity: ltr_cap,
        }
    });
    (left.unwrap(), right)
}

impl KSocketHandle {
    pub fn send_message(&self, msg: KernelValue) -> Option<()> {
        let other = self.other_side.upgrade()?;
        let mut queue = other.queue.lock();

        // check channel capacity
        if queue.len() >= other.capacity {
            self.send_queue_full_event.lock().set_level(true);
            return None;
        }

        queue.push_back(msg);
        other.recv_queue_empty_event.lock().set_level(false);

        Some(())
    }

    pub fn recv_message(&self) -> Option<KernelValue> {
        let mut queue = self.queue.lock();
        match queue.pop_front() {
            Some(el) => {
                self.other_side
                    .upgrade()
                    .map(|o| o.send_queue_full_event.lock().set_level(false));
                Some(el)
            }
            None => {
                // keep level low if eof
                self.recv_queue_empty_event.lock().set_level(!self.is_eof());
                None
            }
        }
    }

    pub fn is_eof(&self) -> bool {
        self.closed_event.lock().level()
    }

    pub fn get_event(&self, ev: SocketEvents) -> Arc<Spinlock<KEvent>> {
        match ev {
            SocketEvents::RecvBufferEmpty => self.recv_queue_empty_event.clone(),
            SocketEvents::SendBufferFull => self.send_queue_full_event.clone(),
            SocketEvents::OtherSideClosed => self.closed_event.clone(),
        }
    }
}

impl Drop for KSocketHandle {
    fn drop(&mut self) {
        if let Some(o) = self.other_side.upgrade() {
            o.closed_event.lock().set_level(true);
            o.recv_queue_empty_event.lock().set_level(false);
        }
    }
}
