use serde::{Deserialize, Serialize};

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    message::MessageHandle,
    object::KernelReference,
    service::{deserialize, serialize, SimpleService},
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
    InvalidRequestForFileType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FSServiceMessageResp<'a> {
    ExpectedQuestion,

    #[serde(borrow)]
    StatResponse(StatResponse<'a>),

    ReadResponse(Option<usize>),

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
    disk: usize,
    file: &str,
    buffer: &'a mut Vec<u8>,
) -> Result<StatResponse<'a>, FSServiceError> {
    let mut fs = SimpleService::with_name("FS");
    serialize(&FSServiceMessage::RunStat(disk, file), buffer);
    fs.call(buffer, &mut Vec::new()).unwrap();

    match deserialize::<Result<FSServiceMessageResp, FSServiceError>>(buffer).unwrap()? {
        FSServiceMessageResp::StatResponse(resp) => Ok(resp),
        _ => todo!(),
    }
}

pub fn read_file_sector(
    disk: usize,
    node: usize,
    sector: u32,
    buffer: &mut Vec<u8>,
) -> Result<Option<MessageHandle>, FSServiceError> {
    let mut fs = SimpleService::with_name("FS");
    serialize(
        &FSServiceMessage::ReadRequest(ReadRequest {
            disk_id: disk,
            node_id: node,
            sector,
        }),
        buffer,
    );
    let mut handles = Vec::with_capacity(1);
    fs.call(buffer, &mut handles).unwrap();
    match deserialize::<Result<FSServiceMessageResp, FSServiceError>>(&buffer).unwrap()? {
        FSServiceMessageResp::ReadResponse(None) => Ok(None),
        FSServiceMessageResp::ReadResponse(Some(_)) => Ok(Some(MessageHandle::from_kref(
            KernelReference::from_id(handles[0]),
        ))),
        _ => todo!(),
    }
}

pub fn read_full_file(
    disk: usize,
    node: usize,
    buffer: &mut Vec<u8>,
) -> Result<Option<MessageHandle>, FSServiceError> {
    let mut fs = SimpleService::with_name("FS");
    serialize(
        &FSServiceMessage::ReadFullFileRequest(ReadFullFileRequest {
            disk_id: disk,
            node_id: node,
        }),
        buffer,
    );
    let mut handles = Vec::with_capacity(1);
    fs.call(buffer, &mut handles).unwrap();
    match deserialize::<Result<FSServiceMessageResp, FSServiceError>>(&buffer).unwrap()? {
        FSServiceMessageResp::ReadResponse(None) => Ok(None),
        FSServiceMessageResp::ReadResponse(Some(_)) => Ok(Some(MessageHandle::from_kref(
            KernelReference::from_id(handles[0]),
        ))),
        _ => todo!(),
    }
}

pub fn get_disks(buffer: &mut Vec<u8>) -> Result<Box<[u64]>, FSServiceError> {
    let mut fs = SimpleService::with_name("FS");
    serialize(&FSServiceMessage::GetDisksRequest, buffer);
    fs.call(buffer, &mut Vec::new());

    match deserialize::<Result<FSServiceMessageResp, FSServiceError>>(buffer).unwrap()? {
        FSServiceMessageResp::GetDisksResponse(d) => Ok(d),
        _ => todo!(),
    }
}
