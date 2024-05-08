use core::num::NonZeroUsize;

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_userspace::event::{EventCallback, EventQueueListenId, KernelEventQueueListenMode};
use spin::Mutex;

use crate::{kassert, syscall::SyscallError};

bitflags::bitflags! {
    pub struct EdgeTrigger: u8 {
        const RISING_EDGE = 1 << 0;
        const FALLING_EDGE = 1 << 1;
    }
}
pub struct KEvent {
    level: bool,
    // If the event is part of a queue we don't want to allow another queue to listen to level-low
    // since that could create a loop which dealocks changing state, the others should just even out at high
    is_queue: bool,
    listeners: Vec<EdgeListener>,
}

pub struct EdgeListener {
    waker: Arc<dyn KEventListener>,
    callback: EventCallback,
    direction: EdgeTrigger,
    oneshot: bool,
}

impl EdgeListener {
    pub fn new(
        waker: Arc<dyn KEventListener>,
        callback: EventCallback,
        direction: EdgeTrigger,
        oneshot: bool,
    ) -> Self {
        Self {
            waker,
            callback,
            direction,
            oneshot,
        }
    }
}

pub enum EdgeDirection {
    Rising,
    Falling,
}

impl KEvent {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            level: false,
            is_queue: false,
            listeners: Vec::new(),
        }))
    }

    pub fn new_queue() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            level: false,
            is_queue: true,
            listeners: Vec::new(),
        }))
    }

    pub fn level(&self) -> bool {
        self.level
    }

    /// Should only be called by the source
    pub fn trigger_edge(&mut self, level: bool) {
        // ignore if setting the same level
        if self.level == level {
            return;
        }

        // rising
        if level {
            self.listeners.retain(|listener| {
                if listener.direction.contains(EdgeTrigger::RISING_EDGE) {
                    listener.waker.trigger_edge(listener.callback, true);
                    !listener.oneshot
                } else {
                    true
                }
            })
        } else {
            self.listeners.retain(|listener| {
                if listener.direction.contains(EdgeTrigger::FALLING_EDGE) {
                    listener.waker.trigger_edge(listener.callback, false);
                    !listener.oneshot
                } else {
                    true
                }
            })
        }
    }

    /// Should only be called by the source
    pub fn set_level(&mut self, level: bool) {
        self.trigger_edge(level);
        self.level = level;
    }

    pub fn listeners(&mut self) -> &mut Vec<EdgeListener> {
        &mut self.listeners
    }
}

pub trait KEventListener: Send + Sync {
    fn trigger_edge(&self, callback: EventCallback, direction: bool);
}

pub struct KEventQueue {
    weak_self: Weak<KEventQueue>,
    event: Arc<Mutex<KEvent>>,
    inner: Mutex<KEventQueueInner>,
}

impl KEventQueue {
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            weak_self: this.clone(),
            event: KEvent::new_queue(),
            inner: Default::default(),
        })
    }

    pub fn event(&self) -> Arc<Mutex<KEvent>> {
        self.event.clone()
    }

    pub fn listen(
        &self,
        event: Arc<Mutex<KEvent>>,
        callback: EventCallback,
        mode: KernelEventQueueListenMode,
    ) -> Result<EventQueueListenId, SyscallError> {
        let mut this = self.inner.lock();
        let mut ev = event.lock();

        // If level low a queue could listen to itself (or some loop) on level low and that could deadlock
        kassert!(
            !(ev.is_queue && mode == KernelEventQueueListenMode::OnLevelLow),
            "Cannot listen to queue with level low"
        );

        let trigger_mode = match mode {
            KernelEventQueueListenMode::OnEdgeHigh => EdgeTrigger::RISING_EDGE,
            KernelEventQueueListenMode::OnEdgeLow => EdgeTrigger::FALLING_EDGE,
            KernelEventQueueListenMode::OnEdge
            | KernelEventQueueListenMode::OnLevelHigh
            | KernelEventQueueListenMode::OnLevelLow => {
                EdgeTrigger::RISING_EDGE | EdgeTrigger::FALLING_EDGE
            }
        };

        let initial_state = match mode {
            KernelEventQueueListenMode::OnLevelHigh => ev.level,
            KernelEventQueueListenMode::OnLevelLow => !ev.level,
            _ => false,
        };

        this.next_callback_id += 1;
        let inner_cbk_id = EventQueueListenId(NonZeroUsize::new(this.next_callback_id).unwrap());

        this.listening_events.insert(
            inner_cbk_id,
            KEventQueueInfo {
                trigger: initial_state,
                is_in_queue: initial_state,
                mode,
                callback,
                upstream: event.clone(),
            },
        );

        if initial_state {
            this.event_queue.push(inner_cbk_id);
            self.set_level(&mut this, true);
        }

        ev.listeners.push(EdgeListener::new(
            self.weak_self.upgrade().unwrap(),
            EventCallback(inner_cbk_id.0),
            trigger_mode,
            false,
        ));
        Ok(inner_cbk_id)
    }

    pub fn unlisten(&self, listen_id: EventQueueListenId) -> Option<()> {
        let mut this = self.inner.lock();
        let event = this.listening_events.remove(&listen_id)?;
        if event.is_in_queue {
            this.event_queue.retain(|e| *e != listen_id);
        }
        let this_ptr: Arc<dyn KEventListener> = self.weak_self.upgrade().unwrap();
        event
            .upstream
            .lock()
            .listeners
            .retain(|e| !Arc::ptr_eq(&e.waker, &this_ptr));
        Some(())
    }

    pub fn try_pop_event(&self) -> Option<EventCallback> {
        let mut this = self.inner.lock();
        loop {
            let Some(ev) = this.event_queue.pop() else {
                // we have exhaused the events set level low
                self.set_level(&mut this, false);
                return None;
            };
            let event = this.listening_events.get_mut(&ev).unwrap();
            event.is_in_queue = false;

            // check that we can trigger
            // a level triggered thing might've detriggered
            if event.trigger {
                let cbk = event.callback;
                match event.mode {
                    KernelEventQueueListenMode::OnLevelLow
                    | KernelEventQueueListenMode::OnLevelHigh => {
                        // if level and triggered, keep in queue
                        event.is_in_queue = true;
                        this.event_queue.push(ev)
                    }
                    _ => (),
                }

                return Some(cbk);
            }
        }
    }

    fn set_level(&self, inner: &mut KEventQueueInner, level: bool) {
        if inner.level != level {
            self.event.lock().set_level(level);
            inner.level = level;
        }
    }
}

#[derive(Default)]
pub struct KEventQueueInner {
    listening_events: HashMap<EventQueueListenId, KEventQueueInfo>,
    event_queue: Vec<EventQueueListenId>,
    level: bool,
    next_callback_id: usize,
}

struct KEventQueueInfo {
    trigger: bool,
    is_in_queue: bool,
    callback: EventCallback,
    mode: KernelEventQueueListenMode,
    upstream: Arc<Mutex<KEvent>>,
}

impl KEventListener for KEventQueue {
    fn trigger_edge(&self, callback: EventCallback, direction: bool) {
        let mut this = self.inner.lock();
        // we send the callbacks matching the inner listen id
        let inner_id = EventQueueListenId(callback.0);
        let info = this
            .listening_events
            .get_mut(&inner_id)
            .expect("the callback should be in the set of listening events");

        // keep in mind that on setup the directions requested were set
        info.trigger = match info.mode {
            KernelEventQueueListenMode::OnEdge => true,
            KernelEventQueueListenMode::OnEdgeHigh => direction,
            KernelEventQueueListenMode::OnEdgeLow => !direction,
            KernelEventQueueListenMode::OnLevelHigh => direction,
            KernelEventQueueListenMode::OnLevelLow => !direction,
        };

        if info.trigger && !info.is_in_queue {
            this.event_queue.push(inner_id);

            if this.level != true {
                this.level = true;
                // prevent a deadlock if a cyclic event loop happens
                drop(this);
                self.event.lock().set_level(true);
            }
        }
    }
}
