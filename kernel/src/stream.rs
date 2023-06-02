use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{collections::BTreeMap, sync::Arc, sync::Weak, vec::Vec};
use crossbeam_queue::ArrayQueue;

use kernel_userspace::stream::StreamMessage;
use spin::Mutex;

use crate::{
    cpu_localstorage::get_task_mgr_current_pid,
    scheduling::{process::PID, taskmanager::TASKMANAGER},
};

pub type STREAM = Arc<ArrayQueue<StreamMessage>>;
pub type STREAMRef = Weak<ArrayQueue<StreamMessage>>;

pub static STREAMS: Mutex<BTreeMap<StreamId, StreamDefinition>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StreamId(pub u64);

impl StreamId {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl From<u64> for StreamId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

pub struct StreamDefinition {
    pub owner: PID,
    pub subscribers: Vec<PID>,
}

pub fn new() -> StreamId {
    let stream_id = StreamId::new();
    let pid = get_task_mgr_current_pid();
    STREAMS.lock().insert(
        stream_id,
        StreamDefinition {
            owner: pid,
            subscribers: Default::default(),
        },
    );
    stream_id
}

pub fn subscribe(id: StreamId) {
    let pid = get_task_mgr_current_pid();

    STREAMS
        .lock()
        .get_mut(&id)
        .and_then(|v| Some(v.subscribers.push(pid)));
}

pub fn push(message: StreamMessage) {
    let id = StreamId::from(message.stream_id);
    let pid = get_task_mgr_current_pid();

    let s = STREAMS.lock();

    let st = s.get(&id).unwrap();

    let msg = Arc::new(message);

    let mut t = TASKMANAGER.lock();
    for sub in &st.subscribers {
        if sub == &pid {
            continue;
        };

        let subscriber = t.processes.get_mut(sub).unwrap();
        subscriber.messages.push_back(msg.clone())
    }
}

pub fn pop() -> Option<Arc<StreamMessage>> {
    let pid = get_task_mgr_current_pid();
    let mut t = TASKMANAGER.lock();
    let proc = t.processes.get_mut(&pid).unwrap();

    proc.messages.pop_front()
}
