use serde::{Deserialize, Serialize};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    ids::ServiceID,
    service::{generate_tracking_number, ServiceMessage, ServiceMessageContainer},
    syscall::{send_and_get_response_service_message, CURRENT_PID},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FSServiceMessage<'a> {
    // DiskID | Path
    RunStat(usize, &'a str),

    StatResponse(StatResponse),
    ReadRequest(ReadRequest),
    ReadFullFileRequest(ReadFullFileRequest),

    #[serde(borrow)]
    ReadResponse(Option<&'a [u8]>),

    GetDisksRequest,
    GetDisksResponse(Vec<u64>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatResponse {
    File(StatResponseFile),
    Folder(StatResponseFolder),
    NotFound,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StatResponseFile {
    pub node_id: usize,
    pub file_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatResponseFolder {
    pub node_id: usize,

    pub children: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReadRequest {
    pub disk_id: usize,
    pub node_id: usize,
    pub sector: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReadFullFileRequest {
    pub disk_id: usize,
    pub node_id: usize,
}

pub fn add_path(folder: &str, file: &str) -> String {
    if file.starts_with('/') {
        return file.to_string();
    }

    let mut path: Vec<&str> = folder.split('/').filter(|a| !a.is_empty()).collect();

    for sect in file.split('/') {
        if sect.is_empty() || sect == "." {
            continue;
        } else if sect == ".." {
            path.pop();
        } else {
            path.push(sect)
        }
    }

    String::from("/") + path.join("/").as_str()
}

pub fn stat(fs_sid: ServiceID, disk: usize, file: &str) -> StatResponse {
    let resp = send_and_get_response_service_message(&ServiceMessage {
        service_id: fs_sid,
        sender_pid: *CURRENT_PID,
        tracking_number: generate_tracking_number(),
        destination: crate::service::SendServiceMessageDest::ToProvider,
        message: crate::service::ServiceMessageType::FS(FSServiceMessage::RunStat(disk, file)),
    })
    .unwrap();

    let msg = resp.get_message().unwrap();

    match msg.message {
        crate::service::ServiceMessageType::FS(FSServiceMessage::StatResponse(resp)) => {
            return resp
        }
        _ => todo!(),
    }
}

pub fn read_file_sector(
    fs_sid: ServiceID,
    disk: usize,
    node: usize,
    sector: u32,
) -> Option<ReadRequestShim> {
    let resp = send_and_get_response_service_message(&ServiceMessage {
        service_id: fs_sid,
        sender_pid: *CURRENT_PID,
        tracking_number: generate_tracking_number(),
        destination: crate::service::SendServiceMessageDest::ToProvider,
        message: crate::service::ServiceMessageType::FS(FSServiceMessage::ReadRequest(
            ReadRequest {
                disk_id: disk,
                node_id: node,
                sector: sector,
            },
        )),
    })
    .unwrap();

    let msg = resp.get_message().unwrap();

    match msg.message {
        crate::service::ServiceMessageType::FS(FSServiceMessage::ReadResponse(data)) => {
            if data.is_none() {
                return None;
            }
            Some(ReadRequestShim { message: resp })
        }
        _ => todo!(),
    }
}

pub fn read_full_file(fs_sid: ServiceID, disk: usize, node: usize) -> Option<ReadRequestShim> {
    let resp = send_and_get_response_service_message(&ServiceMessage {
        service_id: fs_sid,
        sender_pid: *CURRENT_PID,
        tracking_number: generate_tracking_number(),
        destination: crate::service::SendServiceMessageDest::ToProvider,
        message: crate::service::ServiceMessageType::FS(FSServiceMessage::ReadFullFileRequest(
            ReadFullFileRequest {
                disk_id: disk,
                node_id: node,
            },
        )),
    })
    .unwrap();

    let msg = resp.get_message().unwrap();

    match msg.message {
        crate::service::ServiceMessageType::FS(FSServiceMessage::ReadResponse(data)) => {
            if data.is_none() {
                return None;
            }
            Some(ReadRequestShim { message: resp })
        }
        _ => todo!(),
    }
}

pub struct ReadRequestShim {
    message: ServiceMessageContainer,
}

impl ReadRequestShim {
    pub fn get_data(&self) -> &[u8] {
        let m = self.message.get_message().unwrap();
        if let crate::service::ServiceMessageType::FS(FSServiceMessage::ReadResponse(Some(data))) =
            m.message
        {
            data
        } else {
            unreachable!()
        }
    }
}

pub fn get_disks(fs_sid: ServiceID) -> Vec<u64> {
    let resp = send_and_get_response_service_message(&ServiceMessage {
        service_id: fs_sid,
        sender_pid: *CURRENT_PID,
        tracking_number: generate_tracking_number(),
        destination: crate::service::SendServiceMessageDest::ToProvider,
        message: crate::service::ServiceMessageType::FS(FSServiceMessage::GetDisksRequest),
    })
    .unwrap();

    let msg = resp.get_message().unwrap();

    match msg.message {
        crate::service::ServiceMessageType::FS(FSServiceMessage::GetDisksResponse(d)) => d,
        _ => todo!(),
    }
}
