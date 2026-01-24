use core::fmt::Write;

use alloc::{
    borrow::ToOwned,
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_sys::types::SyscallError;
use rkyv::{
    Archive, Archived, Deserialize, Serialize,
    rancor::{Error, Source},
};

use crate::{
    channel::Channel,
    ipc::{IPCChannel, IPCIterator, TypedIPCMessage},
    service::Service,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Archive, Serialize, Deserialize)]
pub struct FSFileId(pub u64);

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum FSRequest {
    StatRoot,
    StatById(FSFileId),
    GetChildren {
        file: FSFileId,
    },
    ReadFile {
        file: FSFileId,
        offset: usize,
        len: usize,
    },
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct FSFile {
    pub id: FSFileId,
    pub file: FSFileType,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum FSFileType {
    File { length: usize },
    Folder,
}

pub struct FSService {
    chan: IPCChannel,
}

impl FSService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self { chan }
    }

    pub fn stat_root(&mut self) -> FSFile {
        self.chan.send(&FSRequest::StatRoot).unwrap();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn stat_by_id(&mut self, file: FSFileId) -> Option<FSFile> {
        self.chan.send(&FSRequest::StatById(file)).unwrap();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn get_children(
        &mut self,
        file: FSFileId,
    ) -> TypedIPCMessage<'_, Archived<Option<HashMap<String, FSFileId>>>> {
        self.chan.send(&FSRequest::GetChildren { file }).unwrap();
        TypedIPCMessage::new(self.chan.recv().unwrap())
    }

    pub fn read_file(
        &mut self,
        file: FSFileId,
        offset: usize,
        len: usize,
    ) -> TypedIPCMessage<'_, Archived<Option<Vec<u8>>>> {
        self.chan
            .send(&FSRequest::ReadFile { file, offset, len })
            .unwrap();
        TypedIPCMessage::new(self.chan.recv().unwrap())
    }
}

pub struct FSServiceExecutor<I: FSServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: FSServiceImpl> FSServiceExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallError::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedFSRequest>()?;

            let err = match msg {
                ArchivedFSRequest::StatRoot => {
                    let res = self.service.stat_root();
                    self.channel.send(&res)
                }
                ArchivedFSRequest::StatById(id) => {
                    let res = self.service.stat_by_id(id.deserialize(des)?);
                    self.channel.send(&res)
                }
                ArchivedFSRequest::GetChildren { file } => {
                    let res = self.service.get_children(file.deserialize(des)?);
                    // panic!("{res:?}");
                    // self.channel.send(&res)
                    // TODO: Why does this break under send???
                    let bytes = rkyv::to_bytes::<Error>(&res).unwrap();
                    self.channel.channel().write(&bytes, &[])
                }
                ArchivedFSRequest::ReadFile { file, offset, len } => {
                    let res = self.service.read_file(
                        file.deserialize(des)?,
                        offset.deserialize(des)?,
                        len.deserialize(des)?,
                    );
                    self.channel.send(&res)
                }
            };
            err.map_err(Error::new)?;
        }
    }
}

pub trait FSServiceImpl {
    fn stat_root(&mut self) -> FSFile;
    fn stat_by_id(&mut self, file: FSFileId) -> Option<FSFile>;
    fn get_children(&mut self, file: FSFileId) -> Option<HashMap<String, FSFileId>>;
    fn read_file(&mut self, file: FSFileId, offset: usize, len: usize) -> Option<Vec<u8>>;
}

/// FSController

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum FSControllerMessage {
    RegisterFilesystem(Channel),
    GetFilesystems { updates: bool },
}

pub struct FSControllerService {
    chan: IPCChannel,
}

impl FSControllerService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self { chan }
    }

    pub fn register_filesystem(&mut self, chan: Channel) {
        self.chan
            .send(&FSControllerMessage::RegisterFilesystem(chan))
            .unwrap();
        self.chan.recv().unwrap().deserialize().unwrap()
    }

    pub fn get_filesystems(&mut self, updates: bool) -> IPCIterator<Service> {
        self.chan
            .send(&FSControllerMessage::GetFilesystems { updates })
            .unwrap();
        let chan: Channel = self.chan.recv().unwrap().deserialize().unwrap();
        IPCIterator::from(IPCChannel::from_channel(chan))
    }
}

pub struct FSControllerExecutor<I: FSControllerImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: FSControllerImpl> FSControllerExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallError::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (msg, des) = msg.access::<ArchivedFSControllerMessage>()?;

            let err = match msg {
                ArchivedFSControllerMessage::RegisterFilesystem(disk) => {
                    self.service
                        .register_filesystem(disk.deserialize(des).unwrap());
                    self.channel.send(&())
                }
                ArchivedFSControllerMessage::GetFilesystems { updates } => {
                    let res = self.service.get_filesystems(*updates);
                    self.channel.send(&res)
                }
            };
            err.map_err(Error::new)?;
        }
    }
}

pub trait FSControllerImpl {
    fn register_filesystem(&mut self, chan: Channel);
    fn get_filesystems(&mut self, updates: bool) -> Channel;
}

pub fn add_path(folder: &str, file: &str) -> String {
    if file.starts_with('/') {
        return file.to_owned();
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

    "/".to_owned() + path.join("/").as_str()
}

pub fn stat_by_path(root: FSFileId, path: &str, fs: &mut FSService) -> Option<FSFile> {
    let mut file = fs.stat_by_id(root)?;
    for sect in path.split('/') {
        if sect.is_empty() {
            continue;
        }
        match file.file {
            FSFileType::File { .. } => return None,
            FSFileType::Folder => {
                let mut children = fs.get_children(file.id);
                let (children, des) = children.access().unwrap();
                let child = match children {
                    rkyv::option::ArchivedOption::None => return None,
                    rkyv::option::ArchivedOption::Some(children) => children.get(sect)?,
                };
                let f = child.deserialize(des).unwrap();
                file = fs.stat_by_id(f)?;
            }
        }
    }
    Some(file)
}

pub fn tree(
    writer: &mut impl Write,
    fs: &mut FSService,
    root: FSFileId,
    prefix: String,
) -> Result<(), core::fmt::Error> {
    let file = fs.stat_by_id(root).unwrap();

    let mut folder = match file.file {
        FSFileType::File { .. } => return Ok(()),
        FSFileType::Folder => fs.get_children(file.id),
    };

    let (children, des) = folder.access().unwrap();
    let children = children.as_ref().unwrap();
    let mut names = children
        .iter()
        .map(|e| (e.0.to_string(), e.1.deserialize(des).unwrap()))
        .collect::<Vec<_>>();

    names.sort_unstable_by(|(a, _), (b, _)| numeric_sort::cmp(a, b));

    if !names.is_empty() {
        for (name, id) in names.iter().take(names.len() - 1) {
            writeln!(writer, "{}├── {}", &prefix, name)?;
            tree(writer, fs, *id, prefix.clone() + "│   ")?;
        }

        let (name, id) = names.last().unwrap();
        writeln!(writer, "{}└── {}", &prefix, name)?;
        tree(writer, fs, *id, prefix.clone() + "    ")?;
    }
    Ok(())
}
