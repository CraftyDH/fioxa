use serde::{Deserialize, Serialize};

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    ids::ServiceID,
    service::{generate_tracking_number, make_message, ServiceMessageDesc},
    syscall::{send_and_get_response_service_message, CURRENT_PID},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FSServiceMessage<'a> {
    // DiskID | Path
    RunStat(usize, &'a str),
    ReadRequest(ReadRequest),
    ReadFullFileRequest(ReadFullFileRequest),

    GetDisksRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FSServiceError {
    NoSuchPartition(u64),
    CouldNotFollowPath,
    FileNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FSServiceMessageResp<'a> {
    ExpectedQuestion,

    StatResponse(StatResponse<'a>),

    #[serde(borrow)]
    ReadResponse(Option<&'a [u8]>),

    GetDisksResponse(Box<[u64]>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatResponse<'a> {
    File(StatResponseFile),
    #[serde(borrow)]
    Folder(StatResponseFolder<'a>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatResponseFile {
    pub node_id: usize,
    pub file_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatResponseFolder<'a> {
    pub node_id: usize,

    #[serde(borrow)]
    pub children: Vec<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadRequest {
    pub disk_id: usize,
    pub node_id: usize,
    pub sector: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub fn stat<'a>(
    fs_sid: ServiceID,
    disk: usize,
    file: &str,
    buffer: &'a mut Vec<u8>,
) -> Result<StatResponse<'a>, FSServiceError> {
    let resp = send_and_get_response_service_message(
        &ServiceMessageDesc {
            service_id: fs_sid,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: crate::service::SendServiceMessageDest::ToProvider,
        },
        &make_message(&FSServiceMessage::RunStat(disk, file), buffer),
    );

    match resp
        .read::<Result<FSServiceMessageResp, FSServiceError>>(buffer)
        .unwrap()?
    {
        FSServiceMessageResp::StatResponse(resp) => Ok(resp),
        _ => todo!(),
    }
}

pub fn read_file_sector(
    fs_sid: ServiceID,
    disk: usize,
    node: usize,
    sector: u32,
    buffer: &mut Vec<u8>,
) -> Result<Option<&[u8]>, FSServiceError> {
    let resp = send_and_get_response_service_message(
        &ServiceMessageDesc {
            service_id: fs_sid,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: crate::service::SendServiceMessageDest::ToProvider,
        },
        &make_message(
            &FSServiceMessage::ReadRequest(ReadRequest {
                disk_id: disk,
                node_id: node,
                sector,
            }),
            buffer,
        ),
    );
    match resp
        .read::<Result<FSServiceMessageResp, FSServiceError>>(buffer)
        .unwrap()?
    {
        FSServiceMessageResp::ReadResponse(data) => Ok(data),
        _ => todo!(),
    }
}

pub fn read_full_file(
    fs_sid: ServiceID,
    disk: usize,
    node: usize,
    buffer: &mut Vec<u8>,
) -> Result<Option<&[u8]>, FSServiceError> {
    let resp = send_and_get_response_service_message(
        &ServiceMessageDesc {
            service_id: fs_sid,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: crate::service::SendServiceMessageDest::ToProvider,
        },
        &make_message(
            &FSServiceMessage::ReadFullFileRequest(ReadFullFileRequest {
                disk_id: disk,
                node_id: node,
            }),
            buffer,
        ),
    );
    match resp
        .read::<Result<FSServiceMessageResp, FSServiceError>>(buffer)
        .unwrap()?
    {
        FSServiceMessageResp::ReadResponse(data) => Ok(data),
        _ => todo!(),
    }
}

pub fn get_disks(fs_sid: ServiceID, buffer: &mut Vec<u8>) -> Result<Box<[u64]>, FSServiceError> {
    let resp = send_and_get_response_service_message(
        &ServiceMessageDesc {
            service_id: fs_sid,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: crate::service::SendServiceMessageDest::ToProvider,
        },
        &make_message(&FSServiceMessage::GetDisksRequest, buffer),
    );

    match resp
        .read::<Result<FSServiceMessageResp, FSServiceError>>(buffer)
        .unwrap()?
    {
        FSServiceMessageResp::GetDisksResponse(d) => Ok(d),
        _ => todo!(),
    }
}
