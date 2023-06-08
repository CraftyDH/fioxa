use serde::{Deserialize, Serialize};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use crate::service::{
    generate_tracking_number, send_and_get_response_sync, MessageType, ServiceResponse, SID,
};

pub const FS_STAT: usize = 0;
pub const FS_READ: usize = 1;
pub const FS_READ_FULL_FILE: usize = 2;
pub const FS_GETDISKS: usize = 3;

pub fn stat_file(fs_sid: SID, disk: usize, path: &str) -> ServiceResponse {
    let file = StatRequest { disk, path };

    let tracking = generate_tracking_number();

    let resp = send_and_get_response_sync(fs_sid, MessageType::Request, tracking, FS_STAT, file, 0);

    resp
}

pub fn read_file_sector<'a>(fs_sid: SID, disk: usize, node: usize, sector: u32) -> ServiceResponse {
    let file = ReadRequest {
        disk_id: disk,
        node_id: node,
        sector,
    };

    let tracking = generate_tracking_number();

    let resp = send_and_get_response_sync(fs_sid, MessageType::Request, tracking, FS_READ, file, 0);

    assert!(resp.get_message_header().data_type == FS_READ);

    resp
}

pub fn read_full_file<'a>(fs_sid: SID, disk: usize, node: usize) -> ServiceResponse {
    let file = ReadFullFileRequest {
        disk_id: disk,
        node_id: node,
    };

    let tracking = generate_tracking_number();

    let resp = send_and_get_response_sync(
        fs_sid,
        MessageType::Request,
        tracking,
        FS_READ_FULL_FILE,
        file,
        0,
    );

    resp
}

pub fn get_disks(fs_sid: SID) -> Vec<u64> {
    let tracking = generate_tracking_number();

    let resp =
        send_and_get_response_sync(fs_sid, MessageType::Request, tracking, FS_GETDISKS, (), 0);

    assert_eq!({ resp.get_message_header().data_type }, FS_GETDISKS);
    resp.get_data_as::<Vec<u64>>().unwrap()
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct StatRequest<'a> {
    pub disk: usize,
    pub path: &'a str,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum StatResponse {
    File(StatResponseFile),
    Folder(StatResponseFolder),
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct StatResponseFile {
    pub node_id: usize,
    pub file_size: usize,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct StatResponseFolder {
    pub node_id: usize,
    pub children: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadRequest {
    pub disk_id: usize,
    pub node_id: usize,
    pub sector: u32,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadFullFileRequest {
    pub disk_id: usize,
    pub node_id: usize,
}

// ReadResponse just bytes
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadResponse<'a> {
    pub data: &'a [u8],
}

// ReadResponse just bytes
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReadResponseVec {
    pub data: Vec<u8>,
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
