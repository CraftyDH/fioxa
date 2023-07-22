use core::{
    char::REPLACEMENT_CHARACTER,
    mem::{size_of, transmute},
    ptr::read_volatile,
    sync::atomic::AtomicUsize,
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

use super::{next_partition_id, FSPartitionDisk, FileSystemDev, PartitionId, PARTITION};

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
    pub partition_id: PartitionId,
    pub bios_parameter_block: BiosParameterBlock,
    pub fat_ebr: FatExtendedBootRecord,
    pub disk: FSPartitionDisk,
    pub file_id_lookup: BTreeMap<usize, FATFile>,
    pub cluster_chain_buffer: BTreeMap<u32, Box<[u8]>>,
}

pub fn next_file_id() -> usize {
    static ID: AtomicUsize = AtomicUsize::new(1);
    ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct FATFile {
    cluster: u32,
    entry_type: FATFileType,
}

#[derive(Debug, Clone)]
pub enum FATFileType {
    Folder(Option<BTreeMap<String, usize>>),
    // Filesize
    File(u32),
}

impl FAT {
    pub fn root_dir_sectors(&self) -> u32 {
        let bpb = self.bios_parameter_block;
        match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => {
                (((bpb.root_dir_entries * 32) + bpb.bytes_per_sector - 1) / bpb.bytes_per_sector)
                    as u32
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
                let mut buf = unsafe { Box::new_uninit_slice(512).assume_init() };

                self.disk.read(fat_buf_offset as usize, 1, &mut buf);

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

    pub fn read_directory(&mut self, mut cluster: u32) -> BTreeMap<String, usize> {
        let mut entries = BTreeMap::new();
        let sectors = self.bios_parameter_block.sectors_per_cluster as u32;
        let mut buffer = vec![0u8; 512 * sectors as usize];
        let mut lfn_buf = String::new();
        while cluster > 0 {
            let sector = self.get_start_sector_of_cluster(cluster);
            self.disk.read(sector as usize, sectors, &mut buffer);

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
        dir_entries: &mut BTreeMap<String, usize>,
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
                    .chain({ lfn.chars_2 }.into_iter())
                    .chain({ lfn.chars_3 }.into_iter());

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
                    entry_type: FATFileType::Folder(None),
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

    fn enumerate_root(&mut self) -> BTreeMap<String, usize> {
        if let Some(_) = self.file_id_lookup.get(&0) {
            todo!();
        }

        let mut children;

        // Fat32 uses a normal cluster directory for root
        if let FatExtendedBootRecord::FAT32(fat32) = self.fat_ebr {
            children = self.read_directory(fat32.root_cluster);
        } else {
            children = BTreeMap::new();
            let buffer = &mut [0u8; 512];

            let mut lfn_buf = String::new();

            for sector in
                self.first_data_sector() - self.root_dir_sectors()..self.first_data_sector()
            {
                self.disk.read(sector as usize, 1, buffer);

                let directory_entry = unsafe {
                    core::slice::from_raw_parts(buffer.as_ptr() as *const DirectoryEntry, 16)
                };

                if self.parse_entries(directory_entry, &mut children, &mut lfn_buf) {
                    break;
                }
            }
        }

        let folder = FATFile {
            cluster: 0,
            entry_type: FATFileType::Folder(Some(children.clone())),
        };
        self.file_id_lookup.insert(0, folder);
        children
    }
}

pub fn read_bios_block(disk: FSPartitionDisk) {
    let buffer = &mut [0u8; 512];
    disk.read(0, 1, buffer);

    let bpb = unsafe { *(buffer.as_ptr() as *const BiosParameterBlock) };

    let mut total_clusters = bpb.total_sectors as usize / bpb.sectors_per_cluster as usize;

    // We need to check extended section
    if total_clusters == 0 {
        total_clusters = bpb.total_sectors_ext as usize / bpb.sectors_per_cluster as usize;
    }

    let mut fat: FAT;
    let partition_id = next_partition_id();

    if total_clusters < 4085 {
        todo!("FAT12 Not supported yet")
    } else if total_clusters < 65535 {
        let fat16ext =
            unsafe { *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT16Ext) };
        fat = FAT {
            partition_id,
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
            partition_id,
            bios_parameter_block: bpb,
            fat_ebr: FatExtendedBootRecord::FAT32(fat32ext),
            file_id_lookup: BTreeMap::new(),
            disk,
            cluster_chain_buffer: Default::default(),
        };
    }

    fat.enumerate_root();
    PARTITION.lock().insert(partition_id, Box::new(fat));
}

impl FileSystemDev for FAT {
    fn get_file_by_id(&mut self, file_id: usize) -> super::VFile {
        let mut fat_file = self.file_id_lookup.get(&file_id).unwrap().clone();
        let res;
        let mut update = false;
        match &mut fat_file.entry_type {
            FATFileType::Folder(f) => {
                if f.is_none() {
                    *f = Some(self.read_directory(fat_file.cluster));
                    update = true;
                }
                res = super::VFile {
                    location: (self.partition_id, file_id),
                    specialized: super::VFileSpecialized::Folder(
                        f.clone()
                            .unwrap()
                            .into_iter()
                            .map(|(a, b)| (a, (self.partition_id, b)))
                            .collect(),
                    ),
                };
            }
            FATFileType::File(f) => {
                return super::VFile {
                    location: (self.partition_id, file_id),
                    specialized: super::VFileSpecialized::File(*f as usize),
                }
            }
        }
        if update {
            self.file_id_lookup.insert(file_id, fat_file);
        }
        res
    }

    fn read_file<'a>(&mut self, file_id: usize, buffer: &'a mut Vec<u8>) -> &'a [u8] {
        let fat_file = self.file_id_lookup.get(&file_id).unwrap();

        let length = match fat_file.entry_type {
            FATFileType::Folder(_) => todo!(),
            FATFileType::File(f) => f,
        };

        let buffer_size = (length as usize + 511) & !511;
        let mut sectors_to_read = (length + 511) / 512;
        let mut buffer_offset = 0;
        let mut cluster = fat_file.cluster;

        buffer.resize(buffer_size, 0);

        println!("READING FROM DISK");

        let mut sectors: Vec<(u32, u32)> = Vec::new();

        let sectors_per_cluster = self.bios_parameter_block.sectors_per_cluster as u32;

        while sectors_to_read > 0 {
            let read_amount = core::cmp::min(sectors_to_read, sectors_per_cluster);
            let file_sector = self.get_start_sector_of_cluster(cluster);

            match sectors.last_mut() {
                Some((start, len)) => {
                    if *start + *len == file_sector {
                        *len += read_amount
                    } else {
                        sectors.push((file_sector, read_amount))
                    }
                }
                None => sectors.push((file_sector, read_amount)),
            }

            sectors_to_read -= read_amount;

            cluster = self.get_next_cluster(cluster);
        }

        println!("START READING FROM DISK");

        for (mut sector, mut len) in sectors {
            while len > 0 {
                let read_amount = core::cmp::min(len, 56);

                self.disk
                    .read(sector as usize, read_amount, &mut buffer[buffer_offset..]);
                sector += read_amount;
                len -= read_amount;
                buffer_offset += read_amount as usize * 512;
            }
        }

        println!("FIN READING FROM DISK");
        &buffer[..length as usize]
    }

    fn read_file_sector(
        &mut self,
        file_id: usize,
        file_sector: usize,
        buffer: &mut [u8; 512],
    ) -> Option<usize> {
        let fat_file = self.file_id_lookup.get(&file_id).unwrap();

        let length = match fat_file.entry_type {
            FATFileType::Folder(_) => todo!(),
            FATFileType::File(f) => f,
        };

        let sectors_to_read = (length + 511) / 512;

        if file_sector == sectors_to_read as usize {
            return None;
        }
        let length = if file_sector + 1 == sectors_to_read as usize {
            (length % 512) as usize
        } else {
            512
        };

        let mut cluster = fat_file.cluster;
        for _ in 0..(file_sector / self.bios_parameter_block.sectors_per_cluster as usize) {
            cluster = self.get_next_cluster(cluster);
        }
        let file_sector = self.get_start_sector_of_cluster(cluster)
            + file_sector as u32 % self.bios_parameter_block.sectors_per_cluster as u32;

        self.disk.read(file_sector as usize, 1, buffer);
        Some(length)
    }
}
