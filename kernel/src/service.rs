use core::sync::atomic::AtomicU64;

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use kernel_userspace::{
    ids::{ProcessID, ServiceID, ThreadID},
    service::{
        PublicServiceMessage, SendError, SendServiceMessageDest, ServiceMessage,
        ServiceMessageContainer, ServiceMessageType, ServiceTrackingNumber,
    },
    syscall::{send_service_message, wait_receive_service_message},
};
use spin::Mutex;

use crate::{cpu_localstorage::get_task_mgr_current_pid, scheduling::taskmanager::PROCESSES};

pub static SERVICES: Mutex<BTreeMap<ServiceID, ServiceInfo>> = Mutex::new(BTreeMap::new());

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceInfo {
    pub owner: ProcessID,
    pub subscribers: Vec<ProcessID>,
}

pub fn new(owner: ProcessID) -> ServiceID {
    static IDS: AtomicU64 = AtomicU64::new(2);

    let id = ServiceID(IDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed));
    SERVICES.lock().insert(
        id,
        ServiceInfo {
            owner: owner,
            subscribers: Vec::new(),
        },
    );
    id.clone()
}

pub fn subscribe(pid: ProcessID, id: ServiceID) {
    SERVICES
        .lock()
        .get_mut(&id)
        .and_then(|v| Some(v.subscribers.push(pid)));
}

pub fn push(current_pid: ProcessID, msg: ServiceMessageContainer) -> Result<(), SendError> {
    let message = msg.get_message().map_err(|_| SendError::ParseError)?;

    let mut s = SERVICES.lock();

    let service = s
        .get_mut(&message.service_id)
        .ok_or(SendError::NoSuchService)?;

    if message.sender_pid != current_pid {
        return Err(SendError::NotYourPID);
    }

    let dest = message.destination;

    let m = Arc::new((message.service_id, message.tracking_number, msg));

    match dest {
        SendServiceMessageDest::ToProvider => {
            let mut p = PROCESSES.lock();
            p.get_mut(&service.owner).unwrap().service_msgs.push_back(m);
        }
        SendServiceMessageDest::ToProcess(pid) => {
            let mut p = PROCESSES.lock();
            p.get_mut(&pid)
                .and_then(|p| Some(p.service_msgs.push_back(m)));
        }
        SendServiceMessageDest::ToSubscribers => {
            let mut processes = PROCESSES.lock();
            for pid in &service.subscribers {
                // TODO: Remove dead subscriber from list
                processes
                    .get_mut(pid)
                    .and_then(|p| Some(p.service_msgs.push_back(m.clone())));
            }
        }
    }

    Ok(())
}

pub fn find_message(
    current_pid: ProcessID,
    current_thread: ThreadID,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) -> Option<usize> {
    let mut processes = PROCESSES.lock();
    let proc = processes.get_mut(&current_pid).unwrap();
    let msg;

    if narrow_by_sid.0 == u64::MAX {
        msg = proc.service_msgs.pop_front()?;
    } else {
        let mut iter = proc.service_msgs.iter();
        let index = if narrow_by_tracking.0 == u64::MAX {
            iter.position(|x| x.0 == narrow_by_sid)?
        } else {
            iter.position(|x| x.0 == narrow_by_sid && x.1 == narrow_by_tracking)?
        };
        msg = proc.service_msgs.remove(index)?;
    }

    let length = msg.2.buffer.len();

    proc.threads
        .get_mut(&current_thread)
        .unwrap()
        .current_message = Some(msg);

    Some(length)
}

pub fn get_message(
    current_pid: ProcessID,
    current_thread: ThreadID,
    buffer: &mut [u8],
) -> Option<()> {
    let mut processes = PROCESSES.lock();
    let proc = processes.get_mut(&current_pid).unwrap();
    let msg = proc
        .threads
        .get_mut(&current_thread)?
        .current_message
        .take()?;

    buffer.copy_from_slice(&msg.2.buffer);
    Some(())
}

pub static PUBLIC_SERVICES: Mutex<BTreeMap<&str, ServiceID>> = Mutex::new(BTreeMap::new());

pub fn start_mgmt() {
    let pid = get_task_mgr_current_pid();
    let sid = ServiceID(1);
    SERVICES.lock().insert(
        sid,
        ServiceInfo {
            owner: pid,
            subscribers: Vec::new(),
        },
    );

    let pid = ProcessID(pid.0);

    loop {
        let query = wait_receive_service_message(sid);

        let message = query.get_message().unwrap();

        let resp = match message.message {
            ServiceMessageType::PublicService(PublicServiceMessage::Request(name)) => {
                let s = PUBLIC_SERVICES.lock();

                let sid = s.get(name);

                ServiceMessageType::PublicService(PublicServiceMessage::Response(sid.copied()))
            }
            _ => ServiceMessageType::UnknownCommand,
        };

        send_service_message(&ServiceMessage {
            service_id: sid,
            sender_pid: pid,
            tracking_number: message.tracking_number,
            destination: SendServiceMessageDest::ToProcess(message.sender_pid),
            message: resp,
        })
        .unwrap();
    }
}
