use alloc::{collections::BTreeMap, sync::Arc, sync::Weak};
use crossbeam_queue::ArrayQueue;

use kernel_userspace::stream::StreamMessage;
use spin::Mutex;

pub type STREAM = Arc<ArrayQueue<StreamMessage>>;
pub type STREAMRef = Weak<ArrayQueue<StreamMessage>>;

pub static STREAMS: Mutex<BTreeMap<&str, Arc<ArrayQueue<StreamMessage>>>> =
    Mutex::new(BTreeMap::new());
