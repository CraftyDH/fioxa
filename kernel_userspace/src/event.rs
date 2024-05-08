use core::num::NonZeroUsize;
use num_derive::{FromPrimitive, ToPrimitive};

use crate::{make_syscall, object::KernelReferenceID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventCallback(pub NonZeroUsize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventQueueListenId(pub NonZeroUsize);

#[derive(FromPrimitive, ToPrimitive)]
pub enum ReceiveMode {
    GetLevel,
    LevelHigh,
    LevelLow,
    Edge,
    EdgeHigh,
    EdgeLow,
}

#[derive(FromPrimitive, ToPrimitive)]
pub enum KernelEventQueueOperation {
    Create,
    GetEvent,
    PopQueue,
    Listen,
    Unlisten,
}

#[derive(FromPrimitive, ToPrimitive, PartialEq, Eq)]
pub enum KernelEventQueueListenMode {
    // Return event on edge trigger
    OnEdge,
    OnEdgeHigh,
    OnEdgeLow,

    // Return event while the level is satisfied
    OnLevelHigh,
    OnLevelLow,
}

pub fn receive_event(event: KernelReferenceID, recv: ReceiveMode) -> bool {
    let ev_id: usize = event.0.get();
    let level: usize;
    unsafe { make_syscall!(crate::syscall::EVENT, ev_id, recv as usize => level) }
    level != 0
}

pub fn event_queue_create() -> KernelReferenceID {
    let id: usize;
    unsafe {
        make_syscall!(crate::syscall::EVENT_QUEUE, KernelEventQueueOperation::Create as usize => id)
    };
    KernelReferenceID::from_usize(id).unwrap()
}

pub fn event_queue_get_event(id: KernelReferenceID) -> KernelReferenceID {
    let event_id: usize;
    unsafe {
        make_syscall!(crate::syscall::EVENT_QUEUE, KernelEventQueueOperation::GetEvent as usize, id.0.get() => event_id)
    };
    KernelReferenceID::from_usize(event_id).unwrap()
}

pub fn event_queue_pop(id: KernelReferenceID) -> Option<EventCallback> {
    let event_id: usize;
    unsafe {
        make_syscall!(crate::syscall::EVENT_QUEUE, KernelEventQueueOperation::PopQueue as usize, id.0.get() => event_id)
    };
    Some(EventCallback(NonZeroUsize::new(event_id)?))
}

pub fn event_queue_listen(
    id: KernelReferenceID,
    event: KernelReferenceID,
    callback: EventCallback,
    mode: KernelEventQueueListenMode,
) -> EventQueueListenId {
    let x: usize;
    unsafe {
        make_syscall!(
            crate::syscall::EVENT_QUEUE,
            KernelEventQueueOperation::Listen as usize,
            id.0.get(),
            event.0.get(),
            callback.0.get(),
            mode as usize => x
        )
    };
    EventQueueListenId(NonZeroUsize::new(x).unwrap())
}

pub fn event_queue_unlisten(id: KernelReferenceID, event: EventQueueListenId) {
    let x: u16;
    unsafe {
        make_syscall!(crate::syscall::EVENT_QUEUE, KernelEventQueueOperation::Unlisten as usize, id.0.get(), event.0.get() => x)
    };
    let _ = x;
}
