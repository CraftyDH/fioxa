use core::sync::atomic::AtomicU64;

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use kernel_userspace::{
    ids::{ProcessID, ServiceID, ThreadID},
    service::{
        PublicServiceMessage, SendError, SendServiceMessageDest, ServiceMessage,
        ServiceMessageContainer, ServiceMessageType, ServiceTrackingNumber,
    },
    syscall::{receive_service_message_blocking, send_service_message},
};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::get_task_mgr_current_pid,
    scheduling::{
        process::{Process, ScheduleStatus},
        taskmanager::{load_new_task, PROCESSES, TASK_QUEUE},
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
    message: Arc<(ServiceID, ServiceTrackingNumber, ServiceMessageContainer)>,
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

            t.register_state.rax = message.2.buffer.len();
            t.current_message = Some(message);
            t.schedule_status = ScheduleStatus::Scheduled;
            TASK_QUEUE.push((proc.pid, tid)).unwrap();
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
        let query = receive_service_message_blocking(sid);

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
