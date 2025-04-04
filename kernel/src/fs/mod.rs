pub mod fat;
pub mod mbr;

use core::{fmt::Debug, ops::ControlFlow, sync::atomic::AtomicU64};

use alloc::{boxed::Box, collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use conquer_once::spin::Lazy;
use kernel_sys::types::SyscallResult;
use kernel_userspace::{
    fs::{
        FSServiceError, FSServiceMessage, FSServiceMessageResp, StatResponse, StatResponseFile,
        StatResponseFolder,
    },
    message::MessageHandle,
    service::{Service, deserialize, serialize},
};

use crate::{
    driver::disk::{DiskBusDriver, DiskDevice},
    fs::mbr::read_partitions,
    mutex::Spinlock,
};

pub static PARTITION: Lazy<Spinlock<BTreeMap<PartitionId, Box<dyn FileSystemDev>>>> =
    Lazy::new(|| Spinlock::new(BTreeMap::new()));
pub static FSDRIVES: Lazy<Spinlock<FileSystemDrives>> = Lazy::new(|| {
    Spinlock::new(FileSystemDrives {
        disks_buses: Default::default(),
    })
});

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
                info!("{:?}", disk.lock().identify());
                read_partitions(disk);
            }
        }
    }
}

pub struct FSPartitionDisk {
    backing_disk: Arc<Spinlock<dyn DiskDevice>>,
    partition_offset: usize,
    partition_length: usize,
}

impl FSPartitionDisk {
    pub fn new(
        backing_disk: Arc<Spinlock<dyn DiskDevice>>,
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

fn with_partition<F, R>(id: PartitionId, f: F) -> Result<R, FSServiceError>
where
    F: FnOnce(&mut Box<dyn FileSystemDev>) -> Result<R, FSServiceError>,
{
    let mut p = PARTITION.lock();
    let p = p
        .get_mut(&id)
        .ok_or(FSServiceError::NoSuchPartition(id.0))?;
    f(p)
}

pub fn get_file_by_id(id: VFileID) -> Result<VFile, FSServiceError> {
    with_partition(id.0, |p| p.get_file_by_id(id.1))
}

pub fn read_file(id: VFileID, buffer: &mut Vec<u8>) -> Result<&[u8], FSServiceError> {
    with_partition(id.0, |p| p.read_file(id.1, buffer))
}

pub fn read_file_sector(
    id: VFileID,
    sector: usize,
    buf: &mut [u8; 512],
) -> Result<Option<usize>, FSServiceError> {
    with_partition(id.0, |p| p.read_file_sector(id.1, sector, buf))
}

pub trait FileSystemDev: Send + Sync {
    fn get_file_by_id(&mut self, file_id: usize) -> Result<VFile, FSServiceError>;

    fn read_file<'a>(
        &mut self,
        file_id: usize,
        buffer: &'a mut Vec<u8>,
    ) -> Result<&'a [u8], FSServiceError>;

    fn read_file_sector(
        &mut self,
        file_id: usize,
        file_sector: usize,
        buffer: &mut [u8; 512],
    ) -> Result<Option<usize>, FSServiceError>;
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

pub fn get_file_from_path(partition_id: PartitionId, path: &str) -> Result<VFile, FSServiceError> {
    let mut file = get_file_by_id((partition_id, 0))?;

    for sect in path.split('/') {
        if sect.is_empty() {
            continue;
        }
        let folder = match file.specialized {
            VFileSpecialized::Folder(f) => f,
            VFileSpecialized::File(_) => {
                return Err(FSServiceError::CouldNotFollowPath);
            }
        };
        let id = folder.get(sect).ok_or(FSServiceError::CouldNotFollowPath)?;
        file = get_file_by_id(*id)?;
    }
    Ok(file)
}

// pub fn tree(folder: VFileID, prefix: String) {
//     let file = get_file_by_id(folder);

//     let folder = match file.specialized {
//         VFileSpecialized::Folder(f) => f,
//         VFileSpecialized::File(_) => return,
//     };

//     let mut it = folder.into_iter().peekable();
//     while let Some((name, node)) = it.next() {
//         let (t, pref) = match it.peek() {
//             Some(_) => ('├', "│   "),
//             None => ('└', "    "),
//         };
//         println!("{}{}── {}", &prefix, t, name);

//         tree(node, prefix.clone() + pref);
//     }
// }

pub fn file_handler() {
    // A bit of a hack to extend the lifetime
    let mut buffer = Vec::with_capacity(0x1000);
    let mut fs_buffer = Vec::new();
    let mut btree_child_buffer = BTreeMap::new();
    let mut sec_buf = [0; 512];

    let mut service = Service::new(
        "FS",
        || (),
        |handle, ()| {
            match handle.read::<0>(&mut buffer, false, false) {
                Ok(_) => (),
                Err(SyscallResult::ChannelEmpty) => return ControlFlow::Break(()),
                err => {
                    error!("Error recv msg: {err:?}");
                    return ControlFlow::Break(());
                }
            }

            let msg = match deserialize(&buffer) {
                Ok(m) => m,
                Err(e) => {
                    error!("{e:?}");
                    return ControlFlow::Break(());
                }
            };
            let res = run_fs_query(msg, &mut fs_buffer, &mut sec_buf, &mut btree_child_buffer);
            match res {
                Ok((a, b)) => {
                    let m = serialize(&Ok::<_, FSServiceError>(a), &mut buffer);
                    match b {
                        Some(h) => handle.write(&m, &[**h.handle()]).assert_ok(),
                        None => handle.write(&m, &[]).assert_ok(),
                    };
                }
                Err(e) => {
                    let m = serialize(&Err::<FSServiceMessageResp, _>(e), &mut buffer);
                    handle.write(&m, &[]).assert_ok()
                }
            }

            core::ops::ControlFlow::Continue(())
        },
    );
    service.run();
}

fn run_fs_query<'a>(
    query: FSServiceMessage,
    buffer: &'a mut Vec<u8>,
    sec_buffer: &'a mut [u8; 512],
    btree_child_buf: &'a mut BTreeMap<String, VFileID>,
) -> Result<(FSServiceMessageResp<'a>, Option<MessageHandle>), FSServiceError> {
    match query {
        FSServiceMessage::RunStat(disk, path) => {
            let file = get_file_from_path(PartitionId(disk as u64), path)?;
            let stat = match file.specialized {
                VFileSpecialized::Folder(children) => {
                    *btree_child_buf = children;
                    let keys = btree_child_buf.keys();
                    StatResponse::Folder(StatResponseFolder {
                        node_id: file.location.1,
                        children: keys.map(|c| c.as_str()).collect(),
                    })
                }
                VFileSpecialized::File(size) => StatResponse::File(StatResponseFile {
                    node_id: file.location.1,
                    file_size: size,
                }),
            };

            Ok((FSServiceMessageResp::StatResponse(stat), None))
        }
        FSServiceMessage::ReadRequest(req) => {
            if let Some(len) = read_file_sector(
                (PartitionId(req.disk_id as u64), req.node_id),
                req.sector as usize,
                sec_buffer,
            )? {
                Ok((
                    FSServiceMessageResp::ReadResponse(Some(len)),
                    Some(MessageHandle::create(&sec_buffer[0..len])),
                ))
            } else {
                Ok((FSServiceMessageResp::ReadResponse(None), None))
            }
        }
        FSServiceMessage::ReadFullFileRequest(req) => {
            let file_vec = read_file((PartitionId(req.disk_id as u64), req.node_id), buffer)?;
            Ok((
                FSServiceMessageResp::ReadResponse(Some(file_vec.len())),
                Some(MessageHandle::create(file_vec)),
            ))
        }
        FSServiceMessage::GetDisksRequest => {
            let disks = PARTITION.lock().keys().map(|p| p.0).collect();
            Ok((FSServiceMessageResp::GetDisksResponse(disks), None))
        }
    }
}
