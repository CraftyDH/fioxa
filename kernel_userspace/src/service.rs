use core::sync::atomic::AtomicU64;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{
    fs::FSServiceMessage,
    ids::{ProcessID, ServiceID},
    input::InputServiceMessage,
    syscall::{get_pid, send_and_wait_response_service_message},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceTrackingNumber(pub u64);

pub fn generate_tracking_number() -> ServiceTrackingNumber {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    ServiceTrackingNumber(n)
}

pub struct ServiceMessageContainer {
    pub buffer: Vec<u8>,
}

impl ServiceMessageContainer {
    pub fn get_message<'a>(&'a self) -> Result<ServiceMessage<'a>, postcard::Error> {
        postcard::from_bytes(&self.buffer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMessage<'a> {
    pub service_id: ServiceID,
    pub sender_pid: ProcessID,
    pub tracking_number: ServiceTrackingNumber,
    pub destination: SendServiceMessageDest,

    #[serde(borrow)]
    pub message: ServiceMessageType<'a>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SendServiceMessageDest {
    // Sends the message to the service provider
    ToProvider,
    // Sends the message to the given process (only allowable for provider)
    ToProcess(ProcessID),
    // Sends the message to all subscribers
    ToSubscribers,
}

#[derive(Debug, Clone, Copy)]
pub enum SendError {
    Ok,
    ParseError,
    NoSuchService,
    NotYourPID,
    FailedToDecodeResponse,
}

impl SendError {
    pub fn try_decode(num: usize) -> Result<(), SendError> {
        match num {
            0 => Ok(()),
            1 => Err(Self::ParseError),
            2 => Err(Self::NoSuchService),
            3 => Err(Self::NotYourPID),
            _ => Err(Self::FailedToDecodeResponse),
        }
    }

    pub fn to_usize(self) -> usize {
        self as usize
    }
}

pub type SendResponse = Result<ServiceTrackingNumber, SendError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceMessageType<'a> {
    Ack,
    UnknownCommand,
    ExpectedQuestion,
    #[serde(borrow)]
    PublicService(PublicServiceMessage<'a>),
    Input(InputServiceMessage),
    Stdout(&'a str),
    StdoutChar(char),

    #[serde(borrow)]
    FS(FSServiceMessage<'a>),

    // ELF BINARY | ARGS
    ElfLoader(&'a [u8], &'a [u8]),
    ElfLoaderResp(ProcessID),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PublicServiceMessage<'a> {
    Request(&'a str),
    Response(Option<ServiceID>),
}

pub fn get_public_service_id(name: &str) -> Option<ServiceID> {
    let resp = send_and_wait_response_service_message(&ServiceMessage {
        service_id: ServiceID(1),
        sender_pid: get_pid(),
        tracking_number: generate_tracking_number(),
        destination: SendServiceMessageDest::ToProvider,
        message: ServiceMessageType::PublicService(PublicServiceMessage::Request(name)),
    })
    .unwrap();

    let msg = resp.get_message().unwrap();

    match msg.message {
        ServiceMessageType::PublicService(PublicServiceMessage::Response(sid)) => sid,
        _ => panic!("Didn't get valid response"),
    }
}
