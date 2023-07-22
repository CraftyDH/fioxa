use core::sync::atomic::AtomicU64;

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use kernel_userspace::{
    ids::{ProcessID, ServiceID, ThreadID},
    service::{
        self, PublicServiceMessage, SendError, SendServiceMessageDest, ServiceMessage,
        ServiceMessageType, ServiceTrackingNumber,
    },
    syscall::{receive_service_message_blocking, send_service_message, spawn_thread},
};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::get_task_mgr_current_pid,
    scheduling::{
        process::{Process, ScheduleStatus},
        taskmanager::{load_new_task, push_task_queue, PROCESSES},
    },
};

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
            owner,
            subscribers: Vec::new(),
        },
    );
    id
}

pub fn subscribe(pid: ProcessID, id: ServiceID) {
    if let Some(v) = SERVICES.lock().get_mut(&id) {
        v.subscribers.push(pid)
    }
}

pub fn push(current_pid: ProcessID, msg: Box<[u8]>) -> Result<(), SendError> {
    let message = service::parse_message(&msg).map_err(|_| SendError::ParseError)?;

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
        SendServiceMessageDest::ToProvider => send_message(service.owner, m),
        SendServiceMessageDest::ToProcess(pid) => send_message(pid, m),
        SendServiceMessageDest::ToSubscribers => {
            for pid in &service.subscribers {
                send_message(*pid, m.clone())?;
            }
            Ok(())
        }
    }
}

fn send_message(
    pid: ProcessID,
    message: Arc<(ServiceID, ServiceTrackingNumber, Box<[u8]>)>,
) -> Result<(), SendError> {
    let mut p = PROCESSES.lock();
    let proc = p.get_mut(&pid).ok_or(SendError::TargetNotExists)?;

    let waiters = &mut proc.waiting_services;

    // Try getting the list asking for a specific message, then the list asking for a specific service, this the list asking for anything
    let tids = match waiters.get_mut(&(message.0, message.1)) {
        Some(t) => Some(t),
        None => match waiters.get_mut(&(message.0, ServiceTrackingNumber(u64::MAX))) {
            Some(t) => Some(t),
            None => waiters.get_mut(&(ServiceID(u64::MAX), ServiceTrackingNumber(u64::MAX))),
        },
    };

    if let Some(tids) = tids {
        if let Some(tid) = tids.pop() {
            let t = proc
                .threads
                .get_mut(&tid)
                .ok_or(SendError::TargetNotExists)?;

            assert!(
                t.schedule_status == ScheduleStatus::WaitingOn(message.0)
                    || t.schedule_status == ScheduleStatus::WaitingOn(ServiceID(u64::MAX))
            );
            assert!(t.current_message.is_none());

            t.register_state.rax = message.2.len();
            t.current_message = Some(message);
            t.schedule_status = ScheduleStatus::Scheduled;
            push_task_queue((proc.pid, tid)).unwrap();
            return Ok(());
        }
    }

    // otherwise add it to the queue
    proc.service_msgs.push_back(message);
    Ok(())
}

pub fn try_find_message(
    current_thread: ThreadID,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
    proc: &mut Process,
) -> Option<usize> {
    let msg = if narrow_by_sid.0 == u64::MAX {
        proc.service_msgs.pop_front()?
    } else {
        let mut iter = proc.service_msgs.iter();
        let index = if narrow_by_tracking.0 == u64::MAX {
            iter.position(|x| x.0 == narrow_by_sid)?
        } else {
            iter.position(|x| x.0 == narrow_by_sid && x.1 == narrow_by_tracking)?
        };
        proc.service_msgs.remove(index)?
    };

    let length = msg.2.len();

    proc.threads
        .get_mut(&current_thread)
        .unwrap()
        .current_message = Some(msg);

    Some(length)
}

pub fn find_message(
    current_pid: ProcessID,
    current_thread: ThreadID,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) -> Option<usize> {
    let mut processes = PROCESSES.lock();
    let proc = processes.get_mut(&current_pid).unwrap();
    try_find_message(current_thread, narrow_by_sid, narrow_by_tracking, proc)
}

pub fn find_or_wait_message(
    stack_frame: &mut InterruptStackFrame,
    reg: &mut Registers,
    current_pid: ProcessID,
    current_thread: ThreadID,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) {
    let mut processes = PROCESSES.lock();
    let proc = processes.get_mut(&current_pid).unwrap();

    if let Some(msg) = try_find_message(current_thread, narrow_by_sid, narrow_by_tracking, proc) {
        reg.rax = msg;
    } else {
        let key = (narrow_by_sid, narrow_by_tracking);
        match proc.waiting_services.get_mut(&key) {
            Some(vec) => vec.push(current_thread),
            None => {
                proc.waiting_services.insert(key, vec![current_thread]);
            }
        }
        reg.rax = 0;
        let t = proc.threads.get_mut(&current_thread).unwrap();
        t.schedule_status = ScheduleStatus::WaitingOn(narrow_by_sid);
        t.save(stack_frame, reg);

        drop(processes);
        load_new_task(stack_frame, reg);
    }
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

    buffer.copy_from_slice(&msg.2);
    Some(())
}

pub static PUBLIC_SERVICES: Mutex<BTreeMap<String, ServiceID>> = Mutex::new(BTreeMap::new());

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

    let mut buffer = Vec::new();
    spawn_thread(move || loop {
        let query = receive_service_message_blocking(sid, &mut buffer).unwrap();

        let resp = match query.message {
            ServiceMessageType::PublicService(PublicServiceMessage::Request(name)) => {
                let s = PUBLIC_SERVICES.lock();

                let sid = s.get(name);

                ServiceMessageType::PublicService(PublicServiceMessage::Response(sid.copied()))
            }
            ServiceMessageType::PublicService(PublicServiceMessage::RegisterPublicService(
                name,
                sid,
            )) => {
                let mut s = PUBLIC_SERVICES.lock();
                s.insert(name.to_string(), sid);
                ServiceMessageType::Ack
            }
            _ => ServiceMessageType::UnknownCommand,
        };

        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: pid,
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
                message: resp,
            },
            &mut buffer,
        )
        .unwrap();
    });
}
