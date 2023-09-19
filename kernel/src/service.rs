use core::sync::atomic::AtomicU64;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use kernel_userspace::{
    ids::{ProcessID, ServiceID},
    service::{
        self, PublicServiceMessage, SendError, SendServiceMessageDest, ServiceMessage,
        ServiceTrackingNumber,
    },
    syscall::{receive_service_message_blocking, send_service_message, spawn_thread},
};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    scheduling::{
        process::{ProcessMessages, SavedThreadState, Thread, ThreadContext},
        taskmanager::{load_new_task, push_task_queue, PROCESSES},
    },
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

pub fn push(current_pid: ProcessID, msg: Box<[u8]>) -> Result<(), SendError> {
    // Read as () to ensure that we can parse the header of any valid message
    let message: ServiceMessage<()> =
        service::parse_message(&msg).map_err(|_| SendError::ParseError)?;

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
    let proc = PROCESSES
        .lock()
        .get(&pid)
        .ok_or(SendError::TargetNotExists)?
        .clone();

    let mut service_messages = proc.service_messages.lock();
    let waiters = &mut service_messages.waiters;

    loop {
        // Try getting the list asking for a specific message, then the list asking for a specific service, this the list asking for anything
        let tid = match waiters.get_mut(&(message.0, message.1)) {
            Some(t) if !t.is_empty() => t.pop().expect("list should have at least 1 element"),
            _ => match waiters.get_mut(&(message.0, ServiceTrackingNumber(u64::MAX))) {
                Some(t) if !t.is_empty() => t.pop().expect("list should have at least 1 element"),
                _ => match waiters.get_mut(&(ServiceID(u64::MAX), ServiceTrackingNumber(u64::MAX)))
                {
                    Some(t) if !t.is_empty() => {
                        t.pop().expect("list should have at least 1 element")
                    }
                    _ => break,
                },
            },
        };

        let t = proc.threads.lock();
        let Some(thread) = t.threads.get(&tid) else {
            // thread doesn't exist anymore, try again
            continue;
        };

        let mut ctx = thread.context.lock();

        match core::mem::replace(&mut *ctx, ThreadContext::Invalid) {
            ThreadContext::WaitingOn(mut state, id)
                if id == message.0 || id == ServiceID(u64::MAX) =>
            {
                state.register_state.rax = message.2.len();
                *ctx = ThreadContext::Scheduled(state, Some(message));
                push_task_queue(Arc::downgrade(thread)).unwrap();
            }
            e => panic!("thread was not waiting it was {e:?}"),
        }
        return Ok(());
    }

    // try to avoid OOM by restricting max packets in queue.
    if service_messages.queue.len() >= 0x10000 {
        println!("queue for {} is full dropping old packets", pid.0);
        service_messages.queue.pop_front();
    }
    // otherwise add it to the queue
    service_messages.queue.push_back(message);
    Ok(())
}

pub fn try_find_message(
    thread: &Thread,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
    messages: &mut ProcessMessages,
) -> Option<usize> {
    let msg = if narrow_by_sid.0 == u64::MAX {
        messages.queue.pop_front()?
    } else {
        let mut iter = messages.queue.iter();
        let index = if narrow_by_tracking.0 == u64::MAX {
            iter.position(|x| x.0 == narrow_by_sid)?
        } else {
            iter.position(|x| x.0 == narrow_by_sid && x.1 == narrow_by_tracking)?
        };
        messages.queue.remove(index)?
    };

    let length = msg.2.len();

    match &mut *thread.context.lock() {
        ThreadContext::Running(m) => *m = Some(msg),
        e => panic!("thread should be running but was {e:?}"),
    }

    Some(length)
}

pub fn find_message(
    thread: &Thread,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) -> Option<usize> {
    try_find_message(
        thread,
        narrow_by_sid,
        narrow_by_tracking,
        &mut thread.process.service_messages.lock(),
    )
}

pub fn find_or_wait_message(
    stack_frame: &mut InterruptStackFrame,
    reg: &mut Registers,
    current_thread: &Thread,
    narrow_by_sid: ServiceID,
    narrow_by_tracking: ServiceTrackingNumber,
) {
    let mut messages = current_thread.process.service_messages.lock();

    if let Some(msg) = try_find_message(
        current_thread,
        narrow_by_sid,
        narrow_by_tracking,
        &mut messages,
    ) {
        reg.rax = msg;
    } else {
        let key = (narrow_by_sid, narrow_by_tracking);
        match messages.waiters.get_mut(&key) {
            Some(vec) => vec.push(current_thread.tid),
            None => {
                messages.waiters.insert(key, vec![current_thread.tid]);
            }
        }
        reg.rax = 0;
        {
            let threads = current_thread.process.threads.lock();
            let t = threads.threads.get(&current_thread.tid).unwrap();
            let mut ctx = t.context.lock();
            match &*ctx {
                ThreadContext::Running(_) => {
                    *ctx = ThreadContext::WaitingOn(
                        SavedThreadState::new(stack_frame, reg),
                        narrow_by_sid,
                    )
                }
                e => panic!("thread was not running it was: {e:?}"),
            }
        }

        load_new_task(stack_frame, reg);
    }
}

pub fn get_message(thread: &Thread, buffer: &mut [u8]) -> Option<()> {
    let mut ctx = thread.context.lock();

    match &mut *ctx {
        ThreadContext::Running(msg) => {
            buffer.copy_from_slice(&msg.take()?.2);
            Some(())
        }
        e => panic!("thread was not running it was: {e:?}"),
    }
}

pub static PUBLIC_SERVICES: Mutex<BTreeMap<String, ServiceID>> = Mutex::new(BTreeMap::new());

pub fn start_mgmt() {
    let pid = CPULocalStorageRW::get_current_pid();
    let sid = ServiceID(1);
    SERVICES.lock().insert(
        sid,
        ServiceInfo {
            owner: pid,
            subscribers: Default::default(),
        },
    );

    let pid = ProcessID(pid.0);

    let mut buffer = Vec::new();
    spawn_thread(move || loop {
        let query = receive_service_message_blocking(sid, &mut buffer).unwrap();

        let resp = match query.message {
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
