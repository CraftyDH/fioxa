use core::{num::NonZeroUsize, sync::atomic::AtomicUsize};

use alloc::{boxed::Box, sync::Arc};
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    message::MessageId,
    service::ServiceTrackingNumber,
};

pub fn create_new_messageid() -> MessageId {
    static ID: AtomicUsize = AtomicUsize::new(1);
    let id = ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    MessageId(NonZeroUsize::new(id).unwrap())
}

#[derive(Debug)]
pub struct KMessage {
    pub id: MessageId,
    pub data: Box<[u8]>,
}

pub struct KMessageProcRefcount {
    pub msg: Arc<KMessage>,
    pub ref_count: usize,
}

pub struct KMessageInFlight {
    pub service_id: ServiceID,
    pub tracking_number: ServiceTrackingNumber,
    pub sender_pid: ProcessID,

    pub descriptor: Arc<KMessage>,
    pub payload: Option<Arc<KMessage>>,
}
