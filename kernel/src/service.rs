use core::sync::atomic::AtomicU64;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    service::{
        make_message, PublicServiceMessage, SendError, SendServiceMessageDest, ServiceMessageDesc,
        ServiceMessageK, ServiceTrackingNumber,
    },
    syscall::{receive_service_message_blocking, send_service_message, spawn_thread},
};
use spin::Mutex;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    message::{KMessage, KMessageProcRefcount},
    scheduling::{process::Thread, taskmanager::PROCESSES},
};

pub static SERVICES: Mutex<BTreeMap<ServiceID, ServiceInfo>> = Mutex::new(BTreeMap::new());

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceInfo {
    pub owner: ProcessID,
    pub subscribers: BTreeSet<ProcessID>,
}

pub fn new(owner: ProcessID) -> ServiceID {
    static IDS: AtomicU64 = AtomicU64::new(2);

    let id = ServiceID(IDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed));
    SERVICES.lock().insert(
        id,
        ServiceInfo {
            owner,
            subscribers: Default::default(),
        },
    );
    id
}

pub fn subscribe(pid: ProcessID, id: ServiceID) {
    if let Some(v) = SERVICES.lock().get_mut(&id) {
        v.subscribers.insert(pid);
    } else {
        todo!("Handle no service existing");
    }
}

pub fn push(current_pid: ProcessID, msg: &ServiceMessageK) -> Result<(), SendError> {
    let mut s = SERVICES.lock();

    let service = s.get_mut(&msg.service_id).ok_or(SendError::NoSuchService)?;

    if msg.sender_pid != current_pid {
        return Err(SendError::NotYourPID);
    }

    let thread = CPULocalStorageRW::get_current_task();

    let dest = msg.destination;

    let data = thread
        .process
        .service_messages
        .lock()
        .messages
        .get(&msg.descriptor)
        .ok_or(SendError::ParseError)?
        .msg
        .clone();

    let m = Arc::new((
        ServiceMessageDesc {
            service_id: msg.service_id,
            sender_pid: msg.sender_pid,
            tracking_number: msg.tracking_number,
            destination: msg.destination,
        },
        data,
    ));

    match dest {
        SendServiceMessageDest::ToProvider => send_message(service.owner, m),
        SendServiceMessageDest::ToProcess(pid) => send_message(pid, m),
        SendServiceMessageDest::ToSubscribers => {
            for pid in &service.subscribers {
                send_message(*pid, m.clone());
            }
        }
    }
    Ok(())
}

fn send_message(pid: ProcessID, message: Arc<(ServiceMessageDesc, Arc<KMessage>)>) {
    let Some(proc) = PROCESSES.lock().get(&pid).cloned() else {
        println!("WARNING: subscribed process died.");
        return;
    };

    let mut service_messages = proc.service_messages.lock();

    let queue = service_messages
        .queue
        .entry(message.0.service_id)
        .or_default();

    // try to avoid OOM by restricting max packets in queue.
    if queue.message_queue.len() >= 0x10000 {
        println!("queue for {} is full dropping old packets", pid.0);
        queue.message_queue.pop_front();
    }
    // otherwise add it to the queue
    queue.message_queue.push_back(message);

    while let Some(thread) = queue.wakers.pop() {
        if let Some(t) = thread.upgrade() {
            t.internal_wake();
        }
    }
}

pub fn try_find_message(
    thread: &Thread,
    sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) -> Option<ServiceMessageK> {
    let mut messages = thread.process.service_messages.lock();

    let queue = messages.queue.entry(sid).or_default();

    let msg = if narrow_by_tracking.0 == u64::MAX {
        queue.message_queue.pop_front()?
    } else {
        let mut iter = queue.message_queue.iter();
        let index = iter.position(|x| x.0.tracking_number == narrow_by_tracking)?;
        queue.message_queue.remove(index).unwrap()
    };

    let mid = msg.1.id;
    messages
        .messages
        .entry(mid)
        .or_insert_with(|| KMessageProcRefcount {
            msg: msg.1.clone(),
            ref_count: 0,
        })
        .ref_count += 1;

    Some(ServiceMessageK {
        service_id: msg.0.service_id,
        sender_pid: msg.0.sender_pid,
        tracking_number: msg.0.tracking_number,
        destination: msg.0.destination,
        descriptor: mid,
    })
}

pub fn service_wait(thread: Arc<Thread>, sid: ServiceID) -> bool {
    let mut messages = thread.process.service_messages.lock();
    let queue = messages.queue.entry(sid).or_default();

    if queue.message_queue.is_empty() {
        queue.wakers.push(Arc::downgrade(&thread));
        true
    } else {
        false
    }
}

pub static PUBLIC_SERVICES: Mutex<BTreeMap<String, ServiceID>> = Mutex::new(BTreeMap::new());

pub fn start_mgmt() {
    let pid = CPULocalStorageRW::get_current_pid();
    let sid = ServiceID(1);

    let mut buffer = Vec::new();
    spawn_thread(move || loop {
        let query = receive_service_message_blocking(sid);

        let resp = match query.read(&mut buffer).unwrap() {
            PublicServiceMessage::Request(name) => {
                let s = PUBLIC_SERVICES.lock();

                let sid = s.get(name);

                PublicServiceMessage::Response(sid.copied())
            }
            PublicServiceMessage::RegisterPublicService(name, sid) => {
                let mut s = PUBLIC_SERVICES.lock();
                s.insert(name.to_string(), sid);
                PublicServiceMessage::Ack
            }
            _ => PublicServiceMessage::UnknownCommand,
        };

        send_service_message(
            &ServiceMessageDesc {
                service_id: sid,
                sender_pid: pid,
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
            },
            &make_message(&resp, &mut buffer),
        );
    });
}
