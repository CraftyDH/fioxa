pub mod fat;
pub mod mbr;

use core::{fmt::Debug, sync::atomic::AtomicU64};

use alloc::{boxed::Box, collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    fs::{
        FSServiceError, FSServiceExecuter, FSServiceImpl, StatResponse, StatResponseFile,
        StatResponseFolder,
    },
    ipc::IPCChannel,
    message::MessageHandle,
    service::ServiceExecutor,
};
use spin::Lazy;

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
    ServiceExecutor::with_name("FS", |chan| {
        sys_process_spawn_thread({
            || match FSServiceExecuter::new(
                IPCChannel::from_channel(chan),
                FSServiceHandler {
                    btree_child_buf: BTreeMap::new(),
                    file_buffer: Vec::new(),
                    sec_buffer: [0u8; 512],
                    disks: None,
                },
            )
            .run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
}

pub struct FSServiceHandler {
    btree_child_buf: BTreeMap<String, VFileID>,
    file_buffer: Vec<u8>,
    sec_buffer: [u8; 512],
    disks: Option<Box<[u64]>>,
}

impl FSServiceImpl for FSServiceHandler {
    fn stat(&mut self, disk: u64, path: &str) -> Result<StatResponse<'_>, FSServiceError> {
        let file = get_file_from_path(PartitionId(disk), path)?;
        let stat = match file.specialized {
            VFileSpecialized::Folder(children) => {
                self.btree_child_buf = children;
                let keys = self.btree_child_buf.keys();
                StatResponse::Folder(StatResponseFolder {
                    node_id: file.location.1,
                    children: keys
                        .map(|c| alloc::borrow::Cow::Borrowed(c.as_str()))
                        .collect(),
                })
            }
            VFileSpecialized::File(size) => StatResponse::File(StatResponseFile {
                node_id: file.location.1 as u64,
                file_size: size as u64,
            }),
        };
        Ok(stat)
    }

    fn read_file_sector(
        &mut self,
        disk: u64,
        node: u64,
        sector: u64,
    ) -> Result<Option<(u64, MessageHandle)>, FSServiceError> {
        if let Some(len) = read_file_sector(
            (PartitionId(disk), node as usize),
            sector as usize,
            &mut self.sec_buffer,
        )? {
            Ok(Some((
                len as u64,
                MessageHandle::create(&self.sec_buffer[0..len]),
            )))
        } else {
            Ok(None)
        }
    }

    fn read_full_file(
        &mut self,
        disk: u64,
        node: u64,
    ) -> Result<(u64, MessageHandle), FSServiceError> {
        let file_vec = read_file((PartitionId(disk), node as usize), &mut self.file_buffer)?;
        Ok((file_vec.len() as u64, MessageHandle::create(file_vec)))
    }

    fn get_disks(&mut self) -> Result<&[u64], FSServiceError> {
        let disks = self
            .disks
            .get_or_insert_with(|| PARTITION.lock().keys().map(|p| p.0).collect());
        Ok(disks)
    }
}
