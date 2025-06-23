use alloc::{
    borrow::Cow,
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use kernel_sys::types::SyscallResult;
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::{AsOwned, InlineAsBox, Map},
};

use crate::{
    ipc::{IPCBox, IPCChannel},
    message::MessageHandle,
};

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum FSServiceMessage<'a> {
    // DiskID | Path
    RunStat(u64, #[rkyv(with = InlineAsBox)] &'a str),
    ReadRequest(ReadRequest),
    ReadFullFileRequest(ReadFullFileRequest),

    GetDisksRequest,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum FSServiceError {
    NoSuchPartition(u64),
    CouldNotFollowPath,
    FileNotFound,
    InvalidRequestForFileType,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum StatResponse<'a> {
    File(StatResponseFile),
    Folder(StatResponseFolder<'a>),
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct StatResponseFile {
    pub node_id: u64,
    pub file_size: u64,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct StatResponseFolder<'a> {
    pub node_id: usize,
    #[rkyv(with = Map<AsOwned>)]
    pub children: Vec<Cow<'a, str>>,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct ReadRequest {
    pub disk_id: u64,
    pub node_id: u64,
    pub sector: u64,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct ReadFullFileRequest {
    pub disk_id: u64,
    pub node_id: u64,
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

    "/".to_string() + path.join("/").as_str()
}

pub struct FSService {
    chan: IPCChannel,
    disks: Option<Box<[u64]>>,
}

impl FSService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self { chan, disks: None }
    }

    pub fn stat(&mut self, disk: u64, path: &str) -> Result<StatResponse<'static>, FSServiceError> {
        self.chan
            .send(&FSServiceMessage::RunStat(disk, path))
            .assert_ok();

        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn read_file_sector(
        &mut self,
        disk: u64,
        node: u64,
        sector: u64,
    ) -> Result<Option<(u64, MessageHandle)>, FSServiceError> {
        self.chan
            .send(&FSServiceMessage::ReadRequest(ReadRequest {
                disk_id: disk,
                node_id: node,
                sector,
            }))
            .assert_ok();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn read_full_file(
        &mut self,
        disk: u64,
        node: u64,
    ) -> Result<(u64, MessageHandle), FSServiceError> {
        self.chan
            .send(&FSServiceMessage::ReadFullFileRequest(
                ReadFullFileRequest {
                    disk_id: disk,
                    node_id: node,
                },
            ))
            .assert_ok();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn get_disks(&mut self) -> Result<&[u64], FSServiceError> {
        if self.disks.is_none() {
            self.chan
                .send(&FSServiceMessage::GetDisksRequest)
                .assert_ok();
            let mut msg = self.chan.recv().unwrap();

            let (msg, des) = msg
                .access::<<Result<IPCBox<'static, [u64]>, FSServiceError> as Archive>::Archived>()
                .unwrap();

            let b = match msg {
                rkyv::result::ArchivedResult::Ok(disks) => Ok(disks.0.deserialize(des).unwrap()),
                rkyv::result::ArchivedResult::Err(err) => Err(err.deserialize(des).unwrap()),
            }?;
            self.disks = Some(b);
        }

        Ok(self.disks.as_ref().unwrap())
    }
}

pub struct FSServiceExecuter<I: FSServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: FSServiceImpl> FSServiceExecuter<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, _) = msg.access::<ArchivedFSServiceMessage>()?;

            let err = match msg {
                ArchivedFSServiceMessage::RunStat(disk, path) => {
                    let res = self.service.stat(disk.to_native(), path);
                    self.channel.send(&res)
                }
                ArchivedFSServiceMessage::ReadRequest(rr) => {
                    let res = self.service.read_file_sector(
                        rr.disk_id.to_native(),
                        rr.node_id.to_native(),
                        rr.sector.to_native(),
                    );
                    self.channel.send(&res)
                }
                ArchivedFSServiceMessage::ReadFullFileRequest(rr) => {
                    let res = self
                        .service
                        .read_full_file(rr.disk_id.to_native(), rr.node_id.to_native());
                    self.channel.send(&res)
                }
                ArchivedFSServiceMessage::GetDisksRequest => {
                    let res = self.service.get_disks();
                    let res = res.map(IPCBox);
                    self.channel.send(&res)
                }
            };
            err.into_err().map_err(Error::new)?;
        }
    }
}

pub trait FSServiceImpl {
    fn stat(&mut self, disk: u64, path: &str) -> Result<StatResponse<'_>, FSServiceError>;

    fn read_file_sector(
        &mut self,
        disk: u64,
        node: u64,
        sector: u64,
    ) -> Result<Option<(u64, MessageHandle)>, FSServiceError>;

    fn read_full_file(
        &mut self,
        disk: u64,
        node: u64,
    ) -> Result<(u64, MessageHandle), FSServiceError>;

    fn get_disks(&mut self) -> Result<&[u64], FSServiceError>;
}
