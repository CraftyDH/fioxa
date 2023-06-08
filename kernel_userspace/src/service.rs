use core::sync::atomic::AtomicU64;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::syscall::{poll_service, service_get_data, service_push_msg, yield_now};

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum MessageType {
    // Owner -> Every Client
    Announcement,
    // Client -> Owner
    Request,
    // Owner -> Client
    Response,
}

#[repr(C, packed)]
pub struct SendMessageHeader {
    pub service_id: SID,
    pub message_type: MessageType,
    // Depends on type:
    // Info: Provides info number (see)
    // Announcement: A number that can be used for keeping track of different threads
    // Request: Any number, a response will respond with this number
    // Response: What ever number was given in the request
    pub tracking_number: u64,
    pub data_length: usize,
    pub data_ptr: *const u8,
    pub data_type: usize,
    // Only used for response
    pub receiver_pid: u64,
}

#[repr(C, packed)]
#[derive(Debug)]
pub struct ReceiveMessageHeader {
    pub service_id: SID,
    pub message_type: MessageType,
    // Depends on type:
    // Announcement: Disregard
    // Request: Any number, a response will respond with this number
    // Response: What ever number was given in the request
    pub tracking_number: u64,
    pub data_length: usize,
    pub data_type: usize,
    pub sender_pid: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SID(pub u64);

pub fn generate_tracking_number() -> u64 {
    static NUMBER: AtomicU64 = AtomicU64::new(0);
    NUMBER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub struct ServiceResponse {
    header: ReceiveMessageHeader,
    buf: Vec<u8>,
}

impl ServiceResponse {
    pub fn new(header: ReceiveMessageHeader, buffer: Vec<u8>) -> Self {
        Self {
            header,
            buf: buffer,
        }
    }

    pub fn get_message_header(&self) -> &ReceiveMessageHeader {
        &self.header
    }

    pub fn get_data_as<'a, T: Deserialize<'a>>(&'a self) -> Result<T, postcard::Error> {
        postcard::from_bytes(&self.buf)
    }
}

pub fn send_service_message<T: Serialize>(
    service: SID,
    ty: MessageType,
    tracking: u64,
    data_type: usize,
    data: T,
    receiver: u64,
) {
    let data = postcard::to_allocvec(&data).unwrap();

    let header = SendMessageHeader {
        service_id: service,
        message_type: ty,
        tracking_number: tracking,
        data_length: data.len(),
        data_ptr: data.as_ptr(),
        data_type: data_type,
        receiver_pid: receiver,
    };

    service_push_msg(header).unwrap();
}

pub fn get_service_messages_sync(id: SID) -> ServiceResponse {
    get_service_response_sync(id, u64::MAX)
}

pub fn get_service_response_sync(id: SID, tracking_number: u64) -> ServiceResponse {
    loop {
        if let Some(msg) = poll_service(id, tracking_number) {
            let mut data_buf = vec![0; msg.data_length];
            service_get_data(&mut data_buf).unwrap();

            return ServiceResponse::new(msg, data_buf);
        } else {
            yield_now();
        }
    }
}

pub fn send_and_get_response_sync<T: Serialize>(
    service: SID,
    ty: MessageType,
    tracking: u64,
    data_type: usize,
    data: T,
    receiver: u64,
) -> ServiceResponse {
    send_service_message(service, ty, tracking, data_type, data, receiver);
    get_service_response_sync(service, tracking)
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ServiceRequestServiceID<'a> {
    pub name: &'a str,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ServiceRequestServiceIDResponse {
    pub sid: SID,
}

pub fn get_public_service_id(name: &str) -> Option<SID> {
    let query = send_and_get_response_sync(
        SID(1),
        MessageType::Request,
        generate_tracking_number(),
        0,
        ServiceRequestServiceID { name },
        0,
    );

    if query.header.data_type != 0 {
        return None;
    }
    query
        .get_data_as::<ServiceRequestServiceIDResponse>()
        .ok()
        .map(|f| f.sid)
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct SpawnProcess<'a, 'b> {
    pub elf: &'a [u8],
    pub args: &'b [u8],
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct SpawnProcessVec<'b> {
    pub elf: Vec<u8>,
    pub args: &'b [u8],
}
