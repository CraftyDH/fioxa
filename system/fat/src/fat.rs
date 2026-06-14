use core::{
    cell::RefCell,
    char::REPLACEMENT_CHARACTER,
    mem::{size_of, transmute},
    ops::ControlFlow,
    ptr::{read_volatile, write_volatile},
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
};
use fioxa_rpc::{client::RPCClient, fs_capnp, server::RPCServer, service::ServiceExecutor};
use hashbrown::HashMap;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{channel::Channel, handle::Handle, mutex::Mutex};

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct BiosParameterBlock {
    _jump: [u8; 3],
    software_name: [u8; 8],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    fat_copies: u8,
    root_dir_entries: u16,
    total_sectors: u16,
    media_type: u8,
    fat_sector_cnt: u16,
    sectors_per_track: u16,
    head_cnt: u16,
    hidden_sectors: u32,
    total_sectors_ext: u32,
}

#[repr(C, packed)]
pub struct DirectoryEntry {
    name: [u8; 8],
    ext: [u8; 3],
    attributes: u8,
    _reserved: u8,
    c_time_tenth: u8,
    c_time: u16,
    c_date: u16,
    a_time: u16,
    first_cluster_hi: u16,
    w_time: u16,
    w_date: u16,
    first_cluster_low: u16,
    size: u32,
}

impl DirectoryEntry {
    pub fn cluster(&self) -> u32 {
        (self.first_cluster_hi as u32) << 16 | self.first_cluster_low as u32
    }

    pub fn set_cluster(&mut self, val: u32) {
        self.first_cluster_hi = (val >> 16) as u16;
        self.first_cluster_low = val as u16;
    }
}

#[repr(C, packed)]
pub struct LongFileName {
    order: u8,
    chars_1: [u16; 5],
    attribute: u8,
    entry_type: u8,
    checksum: u8,
    chars_2: [u16; 6],
    _zero: u16,
    chars_3: [u16; 2],
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct FAT16Ext {
    drive_number: u8,
    flags: u8,
    signature: u8,
    volume_id: u32,
    volume_label: [u8; 11],
    fat_type_label: [u8; 8],
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct FAT32Ext {
    sectors_per_fat: u32,
    flags: u16,
    fat_version: u16,
    root_cluster: u32,
    fat_info: u16,
    backup_sector: u16,
    _reserved: [u8; 12],
    drive_number: u8,
    _reserved1: u8,
    boot_signature: u8,
    volume_id: u32,
    volume_label: [u8; 11],
    fat_type_label: [u8; 8],
}

#[allow(dead_code)]
pub enum FatExtendedBootRecord {
    FAT16(FAT16Ext),
    FAT32(FAT32Ext),
}

impl FatExtendedBootRecord {
    pub fn get_type(&self) -> FatType {
        match self {
            Self::FAT16(_) => FatType::Fat16,
            Self::FAT32(_) => FatType::Fat32,
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
pub struct FAT {
    pub bios_parameter_block: BiosParameterBlock,
    pub fat_ebr: FatExtendedBootRecord,
    pub disk: RefCell<RPCClient<fioxa_rpc::disk_capnp::DiskMessage>>,
    pub disk_cache: RefCell<BTreeMap<u32, Box<[u8]>>>,
    pub root_cluster: u32,
    pub total_clusters: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct FATFile {
    // None if root
    dir_entry_sector: Option<u32>,
    dir_entry_index: u8,
    entry_type: FATFileType,
}

#[derive(Debug, Clone, Copy)]
pub enum FATFileType {
    Folder,
    File,
}

#[derive(Debug, Clone, Copy)]
pub struct ClusterEntry {
    data: u32,
    fat: FatType,
}

impl ClusterEntry {
    pub fn value(&self) -> u32 {
        // ignore upper 4 bits for fat32
        // we need to keep track to set them back
        self.data & 0x0FFFFFFF
    }

    pub fn is_free(&self) -> bool {
        self.value() == 0
    }

    pub fn is_eof(&self) -> bool {
        match self.fat {
            FatType::Fat12 => self.value() >= 0x0FF8,
            FatType::Fat16 => self.value() >= 0xFFF8,
            FatType::Fat32 => self.value() >= 0x0FFFFFF8,
        }
    }

    pub fn get_next(&self) -> Option<u32> {
        if self.is_free() || self.is_eof() {
            return None;
        }
        Some(self.value())
    }

    pub const fn eof(fat: FatType) -> Self {
        let v = match fat {
            FatType::Fat12 => 0x0FF8,
            FatType::Fat16 => 0xFFF8,
            FatType::Fat32 => 0x0FFFFFF8,
        };
        Self { data: v, fat }
    }
}

pub struct ClusterIterator<'a> {
    fat: &'a FAT,
    cluster: Option<u32>,
}

impl<'a> ClusterIterator<'a> {
    pub fn new(fat: &'a FAT, cluster: u32) -> Self {
        Self {
            fat,
            cluster: Some(cluster),
        }
    }
}

impl Iterator for ClusterIterator<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        let cluster = self.cluster.take()?;
        self.cluster = self.fat.get_cluster_entry(cluster).get_next();
        Some(cluster)
    }
}

pub struct FreeClusterIterator<'a> {
    fat: &'a FAT,
    cluster: u32,
}

impl<'a> FreeClusterIterator<'a> {
    pub fn new(fat: &'a FAT) -> Self {
        Self { fat, cluster: 2 }
    }
}

impl Iterator for FreeClusterIterator<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.cluster > self.fat.total_clusters {
                return None;
            }

            let cluster = self.cluster;
            self.cluster += 1;
            if self.fat.get_cluster_entry(cluster).is_free() {
                return Some(cluster);
            }
        }
    }
}

impl FAT {
    pub fn root_dir_sectors(&self) -> u32 {
        let bpb = self.bios_parameter_block;
        match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => {
                (bpb.root_dir_entries * 32).div_ceil(bpb.bytes_per_sector) as u32
            }
            // Fat 32 stores start in fat
            FatExtendedBootRecord::FAT32(_) => 0,
        }
    }

    pub fn first_data_sector(&self) -> u32 {
        let bpb = self.bios_parameter_block;
        match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => {
                bpb.reserved_sectors as u32
                    + bpb.fat_sector_cnt as u32 * bpb.fat_copies as u32
                    + self.root_dir_sectors()
            }
            FatExtendedBootRecord::FAT32(fat32) => {
                bpb.reserved_sectors as u32 + fat32.sectors_per_fat * bpb.fat_copies as u32
            }
        }
    }

    #[expect(dead_code)]
    pub fn get_root_directory_sector(&self) -> u32 {
        match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => self.first_data_sector() - self.root_dir_sectors(),
            FatExtendedBootRecord::FAT32(fat32) => {
                self.get_start_sector_of_cluster(fat32.root_cluster)
            }
        }
    }

    #[track_caller]
    pub fn get_start_sector_of_cluster(&self, cluster: u32) -> u32 {
        assert!(cluster >= 2);
        (cluster - 2) * self.bios_parameter_block.sectors_per_cluster as u32
            + self.first_data_sector()
    }

    #[expect(dead_code)]
    pub fn write_disk_cache(&self, sector: u32, data: &[u8; 512]) {
        self.disk_cache
            .borrow_mut()
            .entry(sector)
            .or_default()
            .copy_from_slice(data);
        self.disk_write(sector, data);
    }

    pub fn disk_read<T>(&self, sector: u32, len: u32, f: impl FnOnce(&[u8]) -> T) -> T {
        let mut read = fioxa_rpc::disk::Read::new_req();
        let mut b = read.init();
        b.set_sector(sector as u64);
        b.set_count(len);
        let r = self.disk.borrow_mut().send(&read.build()).unwrap();
        f(r.get_reply()
            .unwrap()
            .get_message()
            .unwrap()
            .get_data()
            .unwrap())
    }

    pub fn disk_write(&self, sector: u32, data: &[u8]) {
        let mut write = fioxa_rpc::disk::Write::new_req();
        let mut b = write.init();
        b.set_sector(sector as u64);
        b.set_data(data);
        self.disk.borrow_mut().send(&write.build()).unwrap();
    }

    pub fn read_disk_cache<T>(&self, sector: u32, f: impl FnOnce(&[u8; 512]) -> T) -> T {
        let mut dc = self.disk_cache.borrow_mut();
        let data = dc
            .entry(sector)
            .or_insert_with(|| self.disk_read(sector, 1, |buf| buf.into()));

        f((&**data).try_into().unwrap())
    }

    pub fn update_disk_cache<T>(&self, sector: u32, f: impl FnOnce(&mut [u8; 512]) -> T) -> T {
        let mut dc = self.disk_cache.borrow_mut();
        let data = dc
            .entry(sector)
            .or_insert_with(|| self.disk_read(sector, 1, |buf| buf.into()));

        let t = f((&mut **data).try_into().unwrap());
        self.disk_write(sector, data);
        t
    }

    pub fn get_cluster_entry(&self, cluster: u32) -> ClusterEntry {
        let fat = self.fat_ebr.get_type();
        let fat_size = match fat {
            FatType::Fat12 | FatType::Fat16 => 2,
            FatType::Fat32 => 4,
        };
        let bpb = self.bios_parameter_block;

        let fat_buf_offset = cluster / (512 / fat_size) + bpb.reserved_sectors as u32;

        let data = self.read_disk_cache(fat_buf_offset, |fat_buffer| {
            let idx = cluster % (512 / fat_size);

            if fat_size == 4 {
                unsafe { read_volatile((fat_buffer.as_ptr() as *const u32).add(idx as usize)) }
            } else if fat_size == 2 {
                unsafe {
                    read_volatile((fat_buffer.as_ptr() as *const u16).add(idx as usize)) as u32
                }
            } else {
                todo!()
            }
        });
        ClusterEntry { data, fat }
    }

    pub fn set_cluster_entry(&self, cluster: u32, entry: ClusterEntry) {
        let fat = self.fat_ebr.get_type();
        let fat_size = match fat {
            FatType::Fat12 | FatType::Fat16 => 2,
            FatType::Fat32 => 4,
        };
        let bpb = self.bios_parameter_block;

        let fat_buf_offset = cluster / (512 / fat_size) + bpb.reserved_sectors as u32;

        self.update_disk_cache(fat_buf_offset, |fat_buffer| {
            let idx = cluster % (512 / fat_size);

            if fat_size == 4 {
                unsafe {
                    write_volatile(
                        (fat_buffer.as_ptr() as *mut u32).add(idx as usize),
                        entry.data,
                    )
                }
            } else if fat_size == 2 {
                unsafe {
                    write_volatile(
                        (fat_buffer.as_ptr() as *mut u16).add(idx as usize),
                        entry.data as u16,
                    )
                }
            } else {
                todo!()
            }
        });
    }

    pub fn get_dir_entry<T>(&self, file: FATFile, f: impl FnOnce(&DirectoryEntry) -> T) -> T {
        self.read_disk_cache(file.dir_entry_sector.unwrap(), |data| unsafe {
            f(&*data
                .as_ptr()
                .cast::<DirectoryEntry>()
                .add(file.dir_entry_index as usize))
        })
    }

    pub fn update_dir_entry<T>(
        &self,
        file: FATFile,
        f: impl FnOnce(&mut DirectoryEntry) -> T,
    ) -> T {
        self.update_disk_cache(file.dir_entry_sector.unwrap(), |data| unsafe {
            f(&mut *data
                .as_mut_ptr()
                .cast::<DirectoryEntry>()
                .add(file.dir_entry_index as usize))
        })
    }

    fn iterate_dir_entries<T>(
        &self,
        file: FATFile,
        mut f: impl FnMut(u32, &[DirectoryEntry]) -> ControlFlow<T>,
    ) -> Option<T> {
        // Fat32 uses a normal cluster directory for root
        if file.dir_entry_sector.is_none()
            && matches!(self.fat_ebr, FatExtendedBootRecord::FAT16(_))
        {
            for sector in
                self.first_data_sector() - self.root_dir_sectors()..self.first_data_sector()
            {
                let r = self.read_disk_cache(sector, |buf| {
                    let directory_entry = unsafe {
                        core::slice::from_raw_parts(buf.as_ptr() as *const DirectoryEntry, 16)
                    };
                    f(sector, directory_entry)
                });
                match r {
                    ControlFlow::Continue(()) => (),
                    ControlFlow::Break(t) => return Some(t),
                }
            }
            return None;
        }

        let start_cluster = match file.dir_entry_sector {
            Some(_) => self.get_dir_entry(file, |e| e.cluster()),
            None => self.root_cluster,
        };

        let sectors = self.bios_parameter_block.sectors_per_cluster as u32;
        let clusters = ClusterIterator::new(self, start_cluster);
        for cluster in clusters {
            let sector = self.get_start_sector_of_cluster(cluster);
            for sector in sector..sector + sectors {
                let r = self.read_disk_cache(sector, |buf| {
                    let directory_entry = unsafe {
                        core::slice::from_raw_parts(buf.as_ptr() as *const DirectoryEntry, 16)
                    };

                    f(sector, directory_entry)
                });

                match r {
                    ControlFlow::Continue(()) => (),
                    ControlFlow::Break(t) => return Some(t),
                }
            }
        }
        None
    }

    #[expect(dead_code)]
    fn iterate_dir_entries_mut<T>(
        &mut self,
        file: FATFile,
        mut f: impl FnMut(&mut [DirectoryEntry]) -> ControlFlow<T>,
    ) -> Option<T> {
        // Fat32 uses a normal cluster directory for root
        if file.dir_entry_sector.is_none()
            && matches!(self.fat_ebr, FatExtendedBootRecord::FAT16(_))
        {
            for sector in
                self.first_data_sector() - self.root_dir_sectors()..self.first_data_sector()
            {
                let r = self.update_disk_cache(sector, |buf| {
                    let directory_entry = unsafe {
                        core::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut DirectoryEntry, 16)
                    };
                    f(directory_entry)
                });
                match r {
                    ControlFlow::Continue(()) => (),
                    ControlFlow::Break(t) => return Some(t),
                }
            }
            return None;
        }

        let start_cluster = match file.dir_entry_sector {
            Some(_) => self.get_dir_entry(file, |e| e.cluster()),
            None => self.root_cluster,
        };

        let sectors = self.bios_parameter_block.sectors_per_cluster as u32;
        let clusters = ClusterIterator::new(self, start_cluster);
        for cluster in clusters {
            let sector = self.get_start_sector_of_cluster(cluster);
            let r = self.update_disk_cache(sector, |buf| {
                let directory_entry = unsafe {
                    core::slice::from_raw_parts_mut(
                        buf.as_mut_ptr() as *mut DirectoryEntry,
                        16 * sectors as usize,
                    )
                };

                f(directory_entry)
            });

            match r {
                ControlFlow::Continue(()) => (),
                ControlFlow::Break(t) => return Some(t),
            }
        }
        None
    }

    pub fn read_directory(&self, file: FATFile) -> HashMap<String, FATFile> {
        let mut dir_entries = HashMap::new();
        let mut lfn_buf = String::new();
        self.iterate_dir_entries(file, |sector, entries| {
            for (index, entry) in entries.iter().enumerate() {
                // No more entries
                if entry.name[0] == 0 {
                    return ControlFlow::Break(());
                }
                // Unused entry
                if entry.name[0] == 0xE5 {
                    continue;
                }
                // Long file name entry
                if entry.attributes == 0x0F {
                    let lfn: &LongFileName = unsafe { transmute(entry) };
                    let iter = { lfn.chars_1 }
                        .into_iter()
                        .chain(lfn.chars_2)
                        .chain(lfn.chars_3);

                    // The name is null terminated
                    let iter = iter.take_while(|b| *b != 0);

                    let chars = char::decode_utf16(iter)
                        .map(|c| c.unwrap_or(REPLACEMENT_CHARACTER))
                        .collect::<String>();

                    // LFN are supposed to be stored in reverse order
                    // TODO: Actually check lfn.order
                    lfn_buf = chars + lfn_buf.as_str();
                    continue;
                }

                let mut name;
                if lfn_buf.is_empty() {
                    name = String::from_utf8_lossy(&entry.name).trim().to_string();
                    if entry.attributes & 0x10 == 0 {
                        let n = String::from_utf8_lossy(&entry.ext);
                        let n = n.trim();
                        if !n.is_empty() {
                            name += ".";
                            name += n;
                        }
                    };
                } else {
                    name = lfn_buf.clone();
                    lfn_buf.clear();
                }

                if name == "." || name == ".." {
                    continue;
                };

                // Directory
                let entry_type = if entry.attributes & 0x10 == 0x10 {
                    FATFileType::Folder
                } else {
                    FATFileType::File
                };

                let file = FATFile {
                    dir_entry_sector: Some(sector),
                    dir_entry_index: index.try_into().unwrap(),
                    entry_type,
                };
                dir_entries.insert(name, file);
            }
            ControlFlow::Continue(())
        });
        dir_entries
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

// Logic based of https://download.microsoft.com/download/1/6/1/161ba512-40e2-4cc9-843a-923143f3456c/fatgen103.doc
pub fn get_fat_type(bpb: &BiosParameterBlock) -> (FatType, u32) {
    #[allow(clippy::manual_div_ceil)] // we are following the algorithm
    let root_dir_sectors = ((bpb.root_dir_entries as u32 * 32) + (bpb.bytes_per_sector as u32 - 1))
        / bpb.bytes_per_sector as u32;

    let fat_size = if bpb.fat_sector_cnt != 0 {
        bpb.fat_sector_cnt as u32
    } else {
        // This path can only be fat32
        // fat32ext.sectors_per_fat as usize
        let fat32ext = unsafe { *((bpb as *const BiosParameterBlock).add(1) as *const FAT32Ext) };
        return (
            FatType::Fat32,
            fat32ext.sectors_per_fat / bpb.sectors_per_cluster as u32,
        );
    };

    let total_sec_size = if bpb.total_sectors != 0 {
        bpb.total_sectors as u32
    } else {
        bpb.total_sectors_ext
    };

    let data_sectors = total_sec_size
        - ((bpb.reserved_sectors as u32 + (bpb.fat_copies as u32 * fat_size)) + root_dir_sectors);

    let total_clusters = data_sectors / bpb.sectors_per_cluster as u32;

    if total_clusters < 4085 {
        (FatType::Fat12, total_clusters)
    } else if total_clusters < 65525 {
        (FatType::Fat16, total_clusters)
    } else {
        (FatType::Fat32, total_clusters)
    }
}

pub fn read_bios_block(mut disk: RPCClient<fioxa_rpc::disk_capnp::DiskMessage>) {
    let mut req = fioxa_rpc::disk::Read::new_req();
    req.init().set_count(1);
    let buf = disk.send(&req.build()).unwrap();
    let mut buf = buf.get_reply().unwrap();
    let buffer = buf.get_message().unwrap().get_data().unwrap();

    let bios_parameter_block = unsafe { *(buffer.as_ptr() as *const BiosParameterBlock) };

    let (fat_type, total_clusters) = get_fat_type(&bios_parameter_block);

    info!("FAT partition of type: {fat_type:?}");

    let (fat, root) = match fat_type {
        FatType::Fat12 => {
            error!("Fat 12 not supported yet");
            return;
        }
        FatType::Fat16 => {
            let fat16ext = unsafe {
                *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT16Ext)
            };
            let root = FATFile {
                dir_entry_sector: None,
                dir_entry_index: 0,
                entry_type: FATFileType::Folder,
            };
            let fat = FAT {
                bios_parameter_block,
                fat_ebr: FatExtendedBootRecord::FAT16(fat16ext),
                disk: RefCell::new(disk),
                disk_cache: Default::default(),
                root_cluster: 0,
                total_clusters,
            };
            (fat, root)
        }
        FatType::Fat32 => {
            let fat32ext = unsafe {
                *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT32Ext)
            };
            let root = FATFile {
                dir_entry_sector: None,
                dir_entry_index: 0,
                entry_type: FATFileType::Folder,
            };
            let fat = FAT {
                bios_parameter_block,
                fat_ebr: FatExtendedBootRecord::FAT32(fat32ext),
                disk: RefCell::new(disk),
                disk_cache: Default::default(),
                root_cluster: fat32ext.root_cluster,
                total_clusters,
            };
            (fat, root)
        }
    };

    let fat = Arc::new(Mutex::new(fat));
    let cache = Arc::new(Mutex::new(HashMap::new()));

    ServiceExecutor::with_name("FS", |chan| {
        let fat = fat.clone();
        let cache = cache.clone();
        sys_process_spawn_thread(move || {
            RPCServer::new(
                chan,
                FatFolder {
                    fat,
                    cache,
                    file: root,
                },
            )
            .run()
        });
    })
    .run()
    .unwrap();
}

type FatCache = Arc<Mutex<HashMap<String, (Arc<Handle>, fs_capnp::FileType)>>>;

#[derive(Clone)]
struct FatFolder {
    fat: Arc<Mutex<FAT>>,
    cache: FatCache,
    file: FATFile,
}

impl FatFolder {
    fn children(&mut self) -> HashMap<String, FATFile> {
        self.fat.lock().read_directory(self.file)
    }
}

impl fioxa_rpc::fs::FolderService for FatFolder {
    fn get_children<'a>(
        &mut self,
        _req: fioxa_rpc::OwnedReader<'a, fs_capnp::folder_get_children::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::folder_got_children::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let children = self.children();
        let mut children_build = res.init_entries(children.len() as u32);
        for (i, c) in children.iter().enumerate() {
            let mut b = children_build.reborrow().get(i as u32);
            b.set_name(c.0);

            let ty = match c.1.entry_type {
                FATFileType::Folder => fs_capnp::FileType::Folder,
                FATFileType::File => fs_capnp::FileType::File,
            };
            b.set_type(ty);
        }

        Ok(())
    }

    fn open<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, fs_capnp::folder_open::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::folder_opened::Owned>,
        res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let name = req.get_name()?.to_str()?;

        let children = self.children();

        match children.get(name) {
            Some(&file) => {
                let mut cache = self.cache.lock();
                let (handle, ty) = cache
                    .entry_ref(name)
                    .or_insert_with(|| {
                        let (l, r) = Channel::new();

                        let ty = match file.entry_type {
                            FATFileType::Folder => {
                                let fat = self.fat.clone();
                                let cache = self.cache.clone();
                                sys_process_spawn_thread(move || {
                                    ServiceExecutor::from_channel(r, |chan| {
                                        let fat = fat.clone();
                                        let cache = cache.clone();
                                        sys_process_spawn_thread(move || {
                                            RPCServer::new(chan, FatFolder { fat, file, cache })
                                                .run()
                                        });
                                    })
                                    .run()
                                    .unwrap();
                                });
                                fs_capnp::FileType::Folder
                            }
                            FATFileType::File => {
                                let fat = self.fat.clone();
                                sys_process_spawn_thread(move || {
                                    ServiceExecutor::from_channel(r, |chan| {
                                        let fat = fat.clone();
                                        sys_process_spawn_thread(move || {
                                            RPCServer::new(chan, FatFile { fat, file }).run()
                                        });
                                    })
                                    .run()
                                    .unwrap();
                                });
                                fs_capnp::FileType::File
                            }
                        };

                        (l.into_inner().into(), ty)
                    })
                    .clone();

                res.set_type(ty);
                res_handles.add(res.init_capability(), handle);
            }
            None => res.set_type(fs_capnp::FileType::None),
        }

        Ok(())
    }
}

#[derive(Clone)]
struct FatFile {
    fat: Arc<Mutex<FAT>>,
    file: FATFile,
}

impl FatFile {
    fn dir_entry<T>(&self, fat: &FAT, f: impl FnOnce(&DirectoryEntry) -> T) -> T {
        fat.get_dir_entry(self.file, f)
    }

    fn dir_entry_mut<T>(&self, fat: &FAT, f: impl FnOnce(&mut DirectoryEntry) -> T) -> T {
        fat.update_dir_entry(self.file, f)
    }
}

impl fioxa_rpc::fs::FileService for FatFile {
    fn size<'a>(
        &mut self,
        _req: fioxa_rpc::OwnedReader<'a, fs_capnp::file_size::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::file_size_read::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        res.set_size(self.dir_entry(&self.fat.lock(), |e| e.size as u64));
        Ok(())
    }

    fn read<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, fs_capnp::file_read::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::file_data::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let offset = req.get_offset() as usize;
        let len = req.get_len() as usize;
        let fat = self.fat.lock();

        let sectors_per_cluster = fat.bios_parameter_block.sectors_per_cluster as usize;

        let (cluster, size) = self.dir_entry(&fat, |e| (e.cluster(), e.size));
        let cluster_iter = ClusterIterator::new(&fat, cluster);

        let mut read_cursor = offset;
        let read_end = (offset + len).min(size as usize);

        let output = res.init_data((read_end - read_cursor) as u32);
        let mut output_idx = 0;

        for (index, cluster) in cluster_iter.enumerate() {
            let sector_start = index * sectors_per_cluster;
            let sector_start_bytes = sector_start * 512;

            let cluster_sector_start = fat.get_start_sector_of_cluster(cluster) as usize;

            let sector_end = sector_start + sectors_per_cluster;
            let sector_end_bytes = sector_end * 512;

            let r_start = read_cursor.max(sector_start_bytes);
            let r_end = read_end.min(sector_end_bytes);

            if r_start <= r_end && r_start >= sector_start_bytes && r_end <= sector_end_bytes {
                let r_bytes = r_end - r_start;
                let r_start_sector = r_start / 512;
                let r_end_sector = r_end.div_ceil(512);

                let disk_sector = cluster_sector_start + (r_start_sector % sectors_per_cluster);

                fat.disk_read(
                    disk_sector as u32,
                    (r_end_sector - r_start_sector) as u32,
                    |buf| {
                        // chop of prefix and suffix
                        let off_start = r_start & 511;
                        let buf = &buf[off_start..off_start + r_bytes];

                        output[output_idx..output_idx + buf.len()].copy_from_slice(buf);
                        output_idx += buf.len();
                    },
                );
                read_cursor = r_end;
                if read_cursor == read_end {
                    break;
                }
            }
        }

        Ok(())
    }

    fn write<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, fs_capnp::file_write::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        _res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::file_write_resp::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        // WARN: The code needs to allocate cluster, write length, then write data in that order
        // otherwise the QEMU fat driver will not properly accept the changes.

        let offset = req.get_offset() as usize;
        let data = req.get_data()?;
        let mut read_idx = 0;

        let fat = self.fat.lock();
        let sectors_per_cluster = fat.bios_parameter_block.sectors_per_cluster as usize;

        let mut write_cursor = offset;
        let write_end = offset + data.len();

        let (mut start_cluster, size) = self.dir_entry(&fat, |e| (e.cluster(), e.size));

        // we need to update length & and might need to grow
        if size < write_end as u32 {
            // we might need to grow
            let mut free = FreeClusterIterator::new(&fat);

            // allocate first cluster
            let mut cluster_entry = if start_cluster == 0 {
                let first = free.next().unwrap();
                let entry = ClusterEntry {
                    data: 0,
                    fat: fat.fat_ebr.get_type(),
                };
                self.dir_entry_mut(&fat, |e| e.set_cluster(first));
                start_cluster = first;
                entry
            } else {
                fat.get_cluster_entry(start_cluster)
            };

            let max_cluster = write_end.div_ceil(sectors_per_cluster * 512) as u32;
            let mut last_cluster = start_cluster;

            for _ in 1..max_cluster {
                let entry = cluster_entry.get_next();
                match entry {
                    Some(e) => {
                        last_cluster = e;
                        cluster_entry = fat.get_cluster_entry(e);
                    }
                    None => {
                        let next = free.next().unwrap();
                        info!("alloc cluster: {next}");
                        cluster_entry.data = next;
                        fat.set_cluster_entry(last_cluster, cluster_entry);
                        last_cluster = next;
                        // set the entry to be free
                        cluster_entry.data = 0;
                    }
                }
            }

            // mark the last cluster as EOF
            if cluster_entry.is_free() {
                fat.set_cluster_entry(last_cluster, ClusterEntry::eof(cluster_entry.fat));
            }

            // now update the file length size
            self.dir_entry_mut(&fat, |e| e.size = write_end as u32);
        }

        let cluster_iter = ClusterIterator::new(&fat, start_cluster);

        for (index, cluster) in cluster_iter.enumerate() {
            let sector_start = index * sectors_per_cluster;
            let sector_start_bytes = sector_start * 512;

            let cluster_sector_start = fat.get_start_sector_of_cluster(cluster) as usize;

            let sector_end = sector_start + sectors_per_cluster;
            let sector_end_bytes = sector_end * 512;

            let w_start = write_cursor.max(sector_start_bytes);
            let w_end = write_end.min(sector_end_bytes);

            if w_start <= w_end && w_start >= sector_start_bytes && w_end <= sector_end_bytes {
                let w_bytes = w_end - w_start;
                let r_start_sector = w_start / 512;
                let r_end_sector = w_end.div_ceil(512);

                let disk_sector = cluster_sector_start + (r_start_sector % sectors_per_cluster);

                let sector_count = (r_end_sector - r_start_sector) as u32;

                let off_start = w_start & 511;
                let off_end = w_end & 511;
                if off_start > 0 || off_end > 0 {
                    // align
                    let mut block = vec![0; sector_count as usize * 512];

                    if sector_count <= 2 {
                        fat.disk_read(disk_sector as u32, sector_count, |buf| {
                            block.copy_from_slice(buf)
                        });
                    } else {
                        if off_start > 0 {
                            fat.disk_read(disk_sector as u32, 1, |buf| {
                                block[0..512].copy_from_slice(buf);
                            });
                        }
                        if off_end > 0 {
                            fat.disk_read(disk_sector as u32 + sector_count - 1, 1, |buf| {
                                block[(sector_count as usize - 1) * 512
                                    ..sector_count as usize * 512]
                                    .copy_from_slice(buf);
                            });
                        }
                    };

                    let write_region = &mut block[off_start..off_start + w_bytes];

                    write_region.copy_from_slice(&data[read_idx..read_idx + w_bytes]);
                    read_idx += write_region.len();
                    fat.disk_write(disk_sector as u32, &block);
                } else {
                    let block = &data[read_idx..read_idx + w_bytes];
                    read_idx += w_bytes;
                    fat.disk_write(disk_sector as u32, block);
                }
                write_cursor += w_bytes;
                if write_cursor == write_end {
                    break;
                }
            }
        }

        assert_eq!(write_cursor, write_end);

        Ok(())
    }
}
