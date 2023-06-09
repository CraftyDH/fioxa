use core::sync::atomic::AtomicU64;

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use kernel_userspace::service::{
    get_service_messages_sync, send_service_message, MessageType, ReceiveMessageHeader,
    SendMessageHeader, ServiceRequestServiceID, ServiceRequestServiceIDResponse, SpawnProcessVec,
    SID,
};
use spin::Mutex;

use crate::{
    cpu_localstorage::get_task_mgr_current_pid,
    elf,
    scheduling::{
        process::{PID, TID},
        taskmanager::PROCESSES,
    },
};

pub static SERVICES: Mutex<BTreeMap<SID, ServiceInfo>> = Mutex::new(BTreeMap::new());

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceInfo {
    pub owner: PID,
    pub clients: Vec<PID>,
}

pub struct KernelMessageHeader {
    pub service_id: SID,
    pub message_type: MessageType,
    pub tracking_number: u64,
    pub sender_pid: PID,
    pub data_type: usize,
    pub data: Vec<u8>,
}

impl Default for KernelMessageHeader {
    fn default() -> Self {
        Self {
            service_id: SID(0),
            message_type: MessageType::Announcement,
            tracking_number: Default::default(),
            sender_pid: 0.into(),
            data: Default::default(),
            data_type: Default::default(),
        }
    }
}

pub fn new(owner: PID) -> SID {
    static IDS: AtomicU64 = AtomicU64::new(2);

    let id = SID(IDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed));
    SERVICES.lock().insert(
        id,
        ServiceInfo {
            owner: owner,
            clients: Vec::new(),
        },
    );
    id.clone()
}

pub fn subscribe(pid: PID, id: SID) {
    SERVICES
        .lock()
        .get_mut(&id)
        .and_then(|v| Some(v.clients.push(pid)));
}

pub fn push(current_pid: PID, msg: &SendMessageHeader) -> Option<()> {
    let data = unsafe { core::slice::from_raw_parts(msg.data_ptr, msg.data_length) }.to_vec();

    let m = Arc::new(KernelMessageHeader {
        service_id: msg.service_id,
        message_type: msg.message_type,
        tracking_number: msg.tracking_number,
        sender_pid: current_pid,
        data_type: msg.data_type,
        data,
    });
    let mut s = SERVICES.lock();

    let service = s.get_mut(&{ msg.service_id }).unwrap();

    match msg.message_type {
        MessageType::Announcement => {
            if service.owner != current_pid {
                return None;
            }
            let mut processes = PROCESSES.lock();
            for client in &service.clients {
                processes
                    .get_mut(client)
                    .unwrap()
                    .service_msgs
                    .push_back(m.clone());
            }
        }
        MessageType::Request => {
            let mut p = PROCESSES.lock();
            p.get_mut(&service.owner).unwrap().service_msgs.push_back(m);
        }
        MessageType::Response => {
            if service.owner != current_pid {
                return None;
            }
            let mut processes = PROCESSES.lock();
            processes
                .get_mut(&msg.receiver_pid.into())
                .and_then(|p| Some(p.service_msgs.push_back(m)));
        }
    }
    Some(())
}

pub fn pop(
    current_pid: PID,
    current_thread: TID,
    narrow_by_sid: SID,
    narrow_by_tracking: u64,
) -> Option<ReceiveMessageHeader> {
    let mut processes = PROCESSES.lock();
    let proc = processes.get_mut(&current_pid).unwrap();
    let msg;

    if narrow_by_sid.0 == u64::MAX {
        msg = proc.service_msgs.pop_front()?;
    } else {
        let mut iter = proc.service_msgs.iter();
        let index = if narrow_by_tracking == u64::MAX {
            iter.position(|x| x.service_id == narrow_by_sid)?
        } else {
            iter.position(|x| {
                x.service_id == narrow_by_sid && x.tracking_number == narrow_by_tracking
            })?
        };
        msg = proc.service_msgs.remove(index)?;
    }

    // Store
    proc.threads
        .get_mut(&current_thread)
        .and_then(|t| Some(t.current_message = Some(msg.clone())));

    Some(ReceiveMessageHeader {
        service_id: msg.service_id,
        message_type: msg.message_type,
        tracking_number: msg.tracking_number,
        data_length: msg.data.len(),
        data_type: msg.data_type,
        sender_pid: msg.sender_pid.into(),
    })
}

pub fn get_data(current_pid: PID, current_thread: TID, ptr: *mut u8) -> Option<()> {
    let mut p = PROCESSES.lock();
    let proc = p.get_mut(&current_pid)?;
    let thread = proc.threads.get_mut(&current_thread)?;

    let msg = thread.current_message.take()?;

    unsafe {
        let loc = core::slice::from_raw_parts_mut(ptr, msg.data.len());
        loc.copy_from_slice(&msg.data);
    }
    Some(())
}

pub static PUBLIC_SERVICES: Mutex<BTreeMap<&str, SID>> = Mutex::new(BTreeMap::new());

pub fn start_mgmt() {
    SERVICES.lock().insert(
        SID(1),
        ServiceInfo {
            owner: get_task_mgr_current_pid(),
            clients: Vec::new(),
        },
    );

    loop {
        let query = get_service_messages_sync(SID(1));

        let header = query.get_message_header();

        if header.data_type == 0 {
            if let Ok(msg) = query.get_data_as::<ServiceRequestServiceID>() {
                let s = PUBLIC_SERVICES.lock();

                let sid = s.get(msg.name);

                if let Some(sid) = sid {
                    send_service_message(
                        SID(1),
                        MessageType::Response,
                        header.tracking_number,
                        0,
                        ServiceRequestServiceIDResponse { sid: *sid },
                        header.sender_pid,
                    )
                } else {
                    send_service_message(
                        SID(1),
                        MessageType::Response,
                        header.tracking_number,
                        1,
                        (),
                        header.sender_pid,
                    )
                }
            }
        } else if header.data_type == 1 {
            if let Ok(msg) = query.get_data_as::<SpawnProcessVec>() {
                let pid = elf::load_elf(&msg.elf, msg.args);
                send_service_message(
                    SID(1),
                    MessageType::Response,
                    header.tracking_number,
                    0,
                    pid.0,
                    header.sender_pid,
                )
            }
        }
    }
}
