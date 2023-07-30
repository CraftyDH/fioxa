use core::sync::atomic::AtomicU64;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{
    elf::LoadElfError,
    fs::FSServiceMessage,
    ids::{ProcessID, ServiceID},
    input::InputServiceMessage,
    net, pci,
    syscall::{get_pid, send_and_get_response_service_message},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceTrackingNumber(pub u64);

pub fn generate_tracking_number() -> ServiceTrackingNumber {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    ServiceTrackingNumber(n)
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
    TargetNotExists,
    FailedToDecodeResponse,
}

impl SendError {
    pub fn try_decode(num: usize) -> Result<(), SendError> {
        match num {
            0 => Ok(()),
            1 => Err(Self::ParseError),
            2 => Err(Self::NoSuchService),
            3 => Err(Self::NotYourPID),
            4 => Err(Self::TargetNotExists),
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
    #[serde(borrow)]
    ElfLoaderResp(Result<ProcessID, LoadElfError<'a>>),

    InterruptEvent,

    #[serde(borrow)]
    PhysicalNet(net::PhysicalNet<'a>),
    Networking(net::Networking),

    PCIDev(pci::PCIDevCmd),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PublicServiceMessage<'a> {
    Request(&'a str),
    Response(Option<ServiceID>),
    RegisterPublicService(&'a str, ServiceID),
}

pub fn get_public_service_id(name: &str, buffer: &mut Vec<u8>) -> Option<ServiceID> {
    let resp = send_and_get_response_service_message(
        &ServiceMessage {
            service_id: ServiceID(1),
            sender_pid: get_pid(),
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::PublicService(PublicServiceMessage::Request(name)),
        },
        buffer,
    )
    .unwrap();

    match resp.message {
        ServiceMessageType::PublicService(PublicServiceMessage::Response(sid)) => sid,
        _ => panic!("Didn't get valid response"),
    }
}

pub fn register_public_service(name: &str, sid: ServiceID, buffer: &mut Vec<u8>) {
    let ServiceMessageType::Ack = send_and_get_response_service_message(
        &ServiceMessage {
            service_id: ServiceID(1),
            sender_pid: get_pid(),
            tracking_number: generate_tracking_number(),
            destination: SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::PublicService(
                PublicServiceMessage::RegisterPublicService(name, sid),
            ),
        },
        buffer,
    )
    .unwrap().message else {
        todo!()
    };
}

pub fn parse_message(buffer: &[u8]) -> Result<ServiceMessage, postcard::Error> {
    postcard::from_bytes(buffer)
}
