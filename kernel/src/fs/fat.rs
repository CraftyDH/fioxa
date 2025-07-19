use core::{
    char::REPLACEMENT_CHARACTER,
    mem::{size_of, transmute},
    ptr::read_volatile,
    sync::atomic::AtomicU64,
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    fs::{FSControllerService, FSFile, FSFileId, FSFileType, FSServiceExecutor, FSServiceImpl},
    ipc::{IPCChannel, IPCIterator},
};
use spin::Mutex;

use crate::fs::FSPartitionDisk;

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

pub enum FatExtendedBootRecord {
    FAT16(FAT16Ext),
    FAT32(FAT32Ext),
}

#[derive(Debug)]
pub enum DirEntryType {
    Folder,
    // Filesize
    File(u32),
}

pub struct FAT {
    pub bios_parameter_block: BiosParameterBlock,
    pub fat_ebr: FatExtendedBootRecord,
    pub disk: FSPartitionDisk,
    pub file_id_lookup: BTreeMap<FSFileId, FATFile>,
    pub cluster_chain_buffer: BTreeMap<u32, Box<[u8]>>,
}

pub fn next_file_id() -> FSFileId {
    static ID: AtomicU64 = AtomicU64::new(1);
    let id = ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    FSFileId(id)
}

#[derive(Debug, Clone)]
pub struct FATFile {
    cluster: u32,
    entry_type: FATFileType,
}

#[derive(Debug, Clone)]
pub enum FATFileType {
    Folder,
    // Filesize
    File(u32),
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

    pub fn get_root_directory_sector(&self) -> u32 {
        match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => self.first_data_sector() - self.root_dir_sectors(),
            FatExtendedBootRecord::FAT32(fat32) => {
                self.get_start_sector_of_cluster(fat32.root_cluster)
            }
        }
    }

    pub fn get_start_sector_of_cluster(&self, cluster: u32) -> u32 {
        assert!(cluster >= 2);
        (cluster - 2) * self.bios_parameter_block.sectors_per_cluster as u32
            + self.first_data_sector()
    }

    // pub fn get_cluster_from_sector()

    pub fn get_next_cluster(&mut self, cluster: u32) -> u32 {
        let fat_size = match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => 2,
            FatExtendedBootRecord::FAT32(_) => 4,
        };
        let bpb = self.bios_parameter_block;

        let fat_buf_offset = cluster / (512 / fat_size) + bpb.reserved_sectors as u32;

        let fat_buffer = match self.cluster_chain_buffer.get(&fat_buf_offset) {
            Some(b) => b,
            None => {
                let buf = self.disk.read(fat_buf_offset as u64, 1).into_boxed_slice();
                self.cluster_chain_buffer.insert(fat_buf_offset, buf);
                self.cluster_chain_buffer.get(&fat_buf_offset).unwrap()
            }
        };

        let idx = cluster % (512 / fat_size);

        if fat_size == 4 {
            unsafe { read_volatile((fat_buffer.as_ptr() as *const u32).add(idx as usize)) }
        } else if fat_size == 2 {
            unsafe { read_volatile((fat_buffer.as_ptr() as *const u16).add(idx as usize)) as u32 }
        } else {
            todo!()
        }
    }

    pub fn read_directory(&mut self, mut cluster: u32, root: bool) -> HashMap<String, FSFileId> {
        let mut entries = HashMap::new();
        // Fat32 uses a normal cluster directory for root
        if root && matches!(self.fat_ebr, FatExtendedBootRecord::FAT16(_)) {
            let mut lfn_buf = String::new();

            for sector in
                self.first_data_sector() - self.root_dir_sectors()..self.first_data_sector()
            {
                let buffer = self.disk.read(sector as u64, 1);

                let directory_entry = unsafe {
                    core::slice::from_raw_parts(buffer.as_ptr() as *const DirectoryEntry, 16)
                };

                if self.parse_entries(directory_entry, &mut entries, &mut lfn_buf) {
                    break;
                }
            }
            return entries;
        }
        let sectors = self.bios_parameter_block.sectors_per_cluster as u64;
        let mut lfn_buf = String::new();
        while cluster > 0 {
            let sector = self.get_start_sector_of_cluster(cluster);
            let buffer = self.disk.read(sector as u64, sectors);

            let directory_entry = unsafe {
                core::slice::from_raw_parts(
                    buffer.as_ptr() as *const DirectoryEntry,
                    16 * sectors as usize,
                )
            };

            if self.parse_entries(directory_entry, &mut entries, &mut lfn_buf) {
                break;
            }

            cluster = self.get_next_cluster(cluster);
        }
        entries
    }

    fn parse_entries(
        &mut self,
        entries: &[DirectoryEntry],
        dir_entries: &mut HashMap<String, FSFileId>,
        lfn_buf: &mut String,
    ) -> bool {
        for entry in entries {
            // No more entries
            if entry.name[0] == 0 {
                return true;
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
                *lfn_buf = chars + lfn_buf.as_str();
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

            let cluster = (entry.first_cluster_hi as u32) << 8 | entry.first_cluster_low as u32;

            let file_id = next_file_id();
            // Directory
            let file = if entry.attributes & 0x10 == 0x10 {
                FATFile {
                    cluster,
                    entry_type: FATFileType::Folder,
                }
            } else {
                FATFile {
                    cluster,
                    entry_type: FATFileType::File(entry.size),
                }
            };
            dir_entries.insert(name, file_id);
            self.file_id_lookup.insert(file_id, file);
        }
        false
    }

    fn enumerate_root(&mut self) {
        if self.file_id_lookup.contains_key(&FSFileId(0)) {
            panic!("enumerate root should only be called once")
        }

        let folder = FATFile {
            cluster: 0,
            entry_type: FATFileType::Folder,
        };

        self.file_id_lookup.insert(FSFileId(0), folder);
    }
}

pub fn read_bios_block(disk: FSPartitionDisk) {
    let buffer = disk.read(0, 1);

    let bpb = unsafe { *(buffer.as_ptr() as *const BiosParameterBlock) };

    let mut total_clusters = bpb.total_sectors as usize / bpb.sectors_per_cluster as usize;

    // We need to check extended section
    if total_clusters == 0 {
        total_clusters = bpb.total_sectors_ext as usize / bpb.sectors_per_cluster as usize;
    }

    let mut fat: FAT;

    if total_clusters < 4085 {
        todo!("FAT12 Not supported yet")
    } else if total_clusters < 65535 {
        let fat16ext =
            unsafe { *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT16Ext) };
        fat = FAT {
            bios_parameter_block: bpb,
            fat_ebr: FatExtendedBootRecord::FAT16(fat16ext),
            file_id_lookup: BTreeMap::new(),
            disk,
            cluster_chain_buffer: Default::default(),
        };
    } else {
        let fat32ext =
            unsafe { *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT32Ext) };
        fat = FAT {
            bios_parameter_block: bpb,
            fat_ebr: FatExtendedBootRecord::FAT32(fat32ext),
            file_id_lookup: BTreeMap::new(),
            disk,
            cluster_chain_buffer: Default::default(),
        };
    }

    fat.enumerate_root();

    let fat = ArcFat(Arc::new(Mutex::new(fat)));

    let (chan, client) = Channel::new();
    {
        let mut fs_controller =
            FSControllerService::from_channel(IPCChannel::connect("FS_CONTROLLER"));
        fs_controller.register_filesystem(client);
    }

    let chan: IPCIterator<Channel> = IPCChannel::from_channel(chan).into();

    for c in chan {
        let fat = fat.clone();
        sys_process_spawn_thread(move || {
            FSServiceExecutor::new(IPCChannel::from_channel(c), fat)
                .run()
                .unwrap();
        });
    }
}

#[derive(Clone)]

struct ArcFat(Arc<Mutex<FAT>>);

impl FSServiceImpl for ArcFat {
    fn stat_root(&mut self) -> FSFile {
        self.0.lock().stat_root()
    }

    fn stat_by_id(&mut self, file: FSFileId) -> Option<FSFile> {
        self.0.lock().stat_by_id(file)
    }

    fn get_children(&mut self, file: FSFileId) -> Option<HashMap<String, FSFileId>> {
        self.0.lock().get_children(file)
    }

    fn read_file(&mut self, file: FSFileId, offset: usize, len: usize) -> Option<Vec<u8>> {
        self.0.lock().read_file(file, offset, len)
    }
}

impl FSServiceImpl for FAT {
    fn stat_root(&mut self) -> FSFile {
        self.stat_by_id(FSFileId(0)).unwrap()
    }

    fn stat_by_id(&mut self, file_id: FSFileId) -> Option<FSFile> {
        let file = self.file_id_lookup.get(&file_id)?.clone();
        let ty = match file.entry_type {
            FATFileType::Folder => FSFileType::Folder,
            FATFileType::File(size) => FSFileType::File {
                length: size as usize,
            },
        };
        Some(FSFile {
            id: file_id,
            file: ty,
        })
    }

    fn get_children(&mut self, file_id: FSFileId) -> Option<hashbrown::HashMap<String, FSFileId>> {
        let file = self.file_id_lookup.get(&file_id)?.clone();
        match file.entry_type {
            FATFileType::Folder => Some(self.read_directory(file.cluster, file_id.0 == 0)),
            FATFileType::File(_) => None,
        }
    }

    fn read_file(&mut self, file: FSFileId, offset: usize, len: usize) -> Option<Vec<u8>> {
        let fat_file = self.file_id_lookup.get(&file)?;
        let FATFileType::File(file_length) = fat_file.entry_type else {
            return None;
        };

        if offset >= file_length as usize {
            return Some(vec![]);
        }

        // set len to be as much as it can
        let len = len.min(file_length as usize - offset);

        let mut res: Vec<u8> = Vec::with_capacity(len);

        let sectors_per_cluster = self.bios_parameter_block.sectors_per_cluster as u32;

        struct State {
            cluster: u32,
            sector: u32,
            avail: u32,
        }

        let mut state = State {
            cluster: fat_file.cluster,
            sector: self.get_start_sector_of_cluster(fat_file.cluster),
            avail: sectors_per_cluster,
        };

        let consume = |this: &mut Self, state: &mut State, cnt| {
            state.sector += cnt;
            state.avail -= cnt;

            if state.avail == 0 {
                state.cluster = this.get_next_cluster(state.cluster);
                state.sector = this.get_start_sector_of_cluster(state.cluster);
                state.avail = sectors_per_cluster;
            }
        };

        let mut start_sectors = offset as u32 / 512;
        while start_sectors > 0 {
            let min = start_sectors.min(state.avail);
            start_sectors -= min;
            consume(self, &mut state, min);
        }

        // align
        let start_bits = offset as u32 % 512;
        let mut to_read = len;
        if start_bits > 0 {
            res.extend(&self.disk.read(state.sector as u64, 1)[(512 - start_bits) as usize..]);
            consume(self, &mut state, 1);
            to_read -= start_bits as usize;
        }

        while to_read > 0 {
            let max_sectors = to_read.div_ceil(512);
            let read_amount = state.avail.min(max_sectors as u32);
            let read_amount_bytes = (read_amount as usize * 512).min(to_read);
            let read = self.disk.read(state.sector as u64, read_amount as u64);
            res.extend(&read[0..read_amount_bytes]);
            consume(self, &mut state, read_amount);
            to_read -= read_amount_bytes;
        }

        Some(res)
    }
}
