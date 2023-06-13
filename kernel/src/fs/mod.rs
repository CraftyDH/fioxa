pub mod fat;
pub mod mbr;

use core::{fmt::Debug, sync::atomic::AtomicU64};

use alloc::{
    borrow::ToOwned, boxed::Box, collections::BTreeMap, string::String, sync::Arc, vec::Vec,
};
use kernel_userspace::{
    fs::{FSServiceMessage, StatResponse, StatResponseFile, StatResponseFolder},
    service::{SendServiceMessageDest, ServiceMessage, ServiceMessageType},
    syscall::{get_pid, receive_service_message_blocking, send_service_message, service_create},
};
use spin::Mutex;

use crate::{
    driver::disk::{DiskBusDriver, DiskDevice},
    fs::mbr::read_partitions,
    service::PUBLIC_SERVICES,
};

lazy_static::lazy_static! {
    pub static ref PARTITION: Mutex<BTreeMap<PartitionId, Box<dyn FileSystemDev>>> = Mutex::new(BTreeMap::new());
    pub static ref FSDRIVES: Mutex<FileSystemDrives> = Mutex::new(FileSystemDrives { disks_buses: Default::default() });
}
pub struct FileSystemDrives {
    disks_buses: Vec<Box<dyn DiskBusDriver>>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct PartitionId(pub u64);

impl From<u64> for PartitionId {
    fn from(value: u64) -> Self {
        PartitionId(value)
    }
}

pub fn next_partition_id() -> PartitionId {
    static ID: AtomicU64 = AtomicU64::new(0);
    PartitionId(ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed))
}

impl FileSystemDrives {
    pub fn add_device(&mut self, device: Box<dyn DiskBusDriver>) {
        self.disks_buses.push(device);
    }

    pub fn identify(&mut self) {
        for bus in &mut self.disks_buses {
            for disk in bus.get_disks() {
                println!("{:?}", disk.lock().identify());
                read_partitions(disk);
            }
        }
    }
}

pub struct FSPartitionDisk {
    backing_disk: Arc<Mutex<dyn DiskDevice>>,
    partition_offset: usize,
    partition_length: usize,
}

impl FSPartitionDisk {
    pub fn new(
        backing_disk: Arc<Mutex<dyn DiskDevice>>,
        partition_offset: usize,
        partition_length: usize,
    ) -> Self {
        Self {
            backing_disk,
            partition_offset,
            partition_length,
        }
    }

    fn read(&self, sector: usize, sector_count: u32, buffer: &mut [u8]) -> Option<()> {
        assert!(sector + sector_count as usize <= self.partition_length);
        self.backing_disk
            .lock()
            .read(sector + self.partition_offset, sector_count, buffer)
    }
}

pub fn get_file_by_id(id: VFileID) -> VFile {
    let mut p = PARTITION.lock();
    let p = p.get_mut(&id.0).unwrap();
    p.get_file_by_id(id.1)
}

pub fn read_file(id: VFileID) -> Vec<u8> {
    let mut p = PARTITION.lock();
    let p = p.get_mut(&id.0).unwrap();
    p.read_file(id.1)
}

pub fn read_file_sector(id: VFileID, sector: usize, buf: &mut [u8; 512]) -> Option<usize> {
    let mut p = PARTITION.lock();
    let p = p.get_mut(&id.0).unwrap();
    p.read_file_sector(id.1, sector, buf)
}

pub trait FileSystemDev: Send + Sync {
    fn get_file_by_id(&mut self, file_id: usize) -> VFile;

    fn read_file(&mut self, file_id: usize) -> Vec<u8>;

    fn read_file_sector(
        &mut self,
        file_id: usize,
        file_sector: usize,
        buffer: &mut [u8; 512],
    ) -> Option<usize>;
}

impl Debug for dyn FileSystemDev {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("File system")
    }
}

// Partition id, file id
pub type VFileID = (PartitionId, usize);

pub struct VFile {
    pub location: VFileID,
    pub specialized: VFileSpecialized,
}

impl VFile {
    pub fn assume_folder(&self) -> &BTreeMap<String, VFileID> {
        if let VFileSpecialized::Folder(x) = &self.specialized {
            return x;
        }
        panic!("Assume folder wasn't given a folder")
    }
}

pub enum VFileSpecialized {
    Folder(BTreeMap<String, VFileID>),
    File(usize),
}

pub fn get_file_from_path(partition_id: PartitionId, path: &str) -> Option<VFile> {
    let mut file = get_file_by_id((partition_id, 0));

    for sect in path.split('/') {
        if sect == "" {
            continue;
        }
        let folder = match file.specialized {
            VFileSpecialized::Folder(f) => f,
            VFileSpecialized::File(_) => {
                println!("Not a directory");
                return None;
            }
        };
        let id = folder.get(sect)?;
        file = get_file_by_id(*id);
    }
    Some(file)
}

pub fn tree(folder: VFileID, prefix: String) {
    let file = get_file_by_id(folder);

    let folder = match file.specialized {
        VFileSpecialized::Folder(f) => f,
        VFileSpecialized::File(_) => return,
    };

    let mut it = folder.into_iter().peekable();
    while let Some((name, node)) = it.next() {
        let (t, pref) = match it.peek() {
            Some(_) => ('├', "│   "),
            None => ('└', "    "),
        };
        println!("{}{}── {}", &prefix, t, name);

        tree(node, prefix.clone() + pref);
    }
}

pub fn file_handler() {
    let sid = service_create();
    let pid = get_pid();
    PUBLIC_SERVICES.lock().insert("FS", sid);

    let mut buffer = [0u8; 512];

    // A bit of a hack to extend the lifetime
    let mut file_vec;
    loop {
        let m = receive_service_message_blocking(sid);

        let query = m.get_message().unwrap();

        let resp = match query.message {
            ServiceMessageType::FS(fs) => match fs {
                FSServiceMessage::RunStat(disk, path) => {
                    if let Some(file) = get_file_from_path(PartitionId(disk as u64), path) {
                        let stat = match file.specialized {
                            VFileSpecialized::Folder(children) => {
                                StatResponse::Folder(StatResponseFolder {
                                    node_id: file.location.1,
                                    children: children.keys().map(|c| c.to_owned()).collect(),
                                })
                            }
                            VFileSpecialized::File(size) => StatResponse::File(StatResponseFile {
                                node_id: file.location.1,
                                file_size: size,
                            }),
                        };

                        ServiceMessageType::FS(FSServiceMessage::StatResponse(stat))
                    } else {
                        ServiceMessageType::FS(FSServiceMessage::StatResponse(
                            StatResponse::NotFound,
                        ))
                    }
                }
                FSServiceMessage::ReadRequest(req) => {
                    if let Some(len) = read_file_sector(
                        (PartitionId(req.disk_id as u64), req.node_id as usize),
                        req.sector as usize,
                        &mut buffer,
                    ) {
                        ServiceMessageType::FS(FSServiceMessage::ReadResponse(Some(
                            &buffer[0..len],
                        )))
                    } else {
                        ServiceMessageType::FS(FSServiceMessage::ReadResponse(None))
                    }
                }
                FSServiceMessage::ReadFullFileRequest(req) => {
                    file_vec = read_file((PartitionId(req.disk_id as u64), req.node_id as usize));
                    ServiceMessageType::FS(FSServiceMessage::ReadResponse(Some(&file_vec)))
                }
                FSServiceMessage::GetDisksRequest => {
                    let disks = PARTITION.lock().keys().map(|p| p.0).collect();
                    ServiceMessageType::FS(FSServiceMessage::GetDisksResponse(disks))
                }
                _ => ServiceMessageType::ExpectedQuestion,
            },
            _ => ServiceMessageType::UnknownCommand,
        };
        send_service_message(&ServiceMessage {
            service_id: sid,
            sender_pid: pid,
            tracking_number: query.tracking_number,
            destination: SendServiceMessageDest::ToProcess(query.sender_pid),
            message: resp,
        })
        .unwrap();
    }
}
