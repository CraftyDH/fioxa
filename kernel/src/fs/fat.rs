use core::{
    char::REPLACEMENT_CHARACTER,
    mem::{size_of, transmute},
    ptr::read_volatile,
};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use super::FSPartitionDisk;

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

#[derive(Debug)]
pub struct DirEntry {
    name: String,
    cluster: u32,
    entry_type: DirEntryType,
}

pub struct FAT {
    pub bios_parameter_block: BiosParameterBlock,
    pub fat_ebr: FatExtendedBootRecord,
    pub disk: FSPartitionDisk,
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

    pub fn get_next_cluster(&self, cluster: u32) -> u32 {
        let fat_size = match self.fat_ebr {
            FatExtendedBootRecord::FAT16(_) => 2,
            FatExtendedBootRecord::FAT32(_) => 4,
        };
        let bpb = self.bios_parameter_block;

        let fat_buf_offset = cluster / (512 / fat_size) + bpb.reserved_sectors as u32;

        let fat_buffer = &mut [0u8; 512];

        self.disk.read(fat_buf_offset as usize, 1, fat_buffer);

        let idx = cluster % (512 / fat_size);

        if fat_size == 4 {
            return unsafe { *(fat_buffer.as_ptr() as *const u32).add(idx as usize) };
        } else if fat_size == 2 {
            return unsafe { read_volatile((fat_buffer.as_ptr() as *const u16).add(idx as usize)) }
                as u32;
        } else {
            todo!()
        }
    }

    pub fn read_root_directory(&self) -> Vec<DirEntry> {
        // Fat32 uses a normal cluster directory for root
        if let FatExtendedBootRecord::FAT32(fat32) = self.fat_ebr {
            return self.read_directory(fat32.root_cluster);
        }
        let mut entries = Vec::new();
        let buffer = &mut [0u8; 512];

        let mut lfn_buf = String::new();

        for sector in self.first_data_sector() - self.root_dir_sectors()..self.first_data_sector() {
            self.disk.read(sector as usize, 1, buffer);

            let directory_entry = unsafe {
                core::slice::from_raw_parts(buffer.as_ptr() as *const DirectoryEntry, 16)
            };

            if self.parse_entries(directory_entry, &mut entries, &mut lfn_buf) {
                break;
            }
        }
        entries
    }

    fn parse_entries(
        &self,
        entries: &[DirectoryEntry],
        dir_entries: &mut Vec<DirEntry>,
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
                    name += ".";
                    name += String::from_utf8_lossy(&entry.ext).trim()
                };
            } else {
                name = lfn_buf.clone();
                lfn_buf.clear();
            }

            if name == "." || name == ".." {
                continue;
            };

            let cluster = (entry.first_cluster_hi as u32) << 8 | entry.first_cluster_low as u32;
            // Directory
            if entry.attributes & 0x10 == 0x10 {
                dir_entries.push(DirEntry {
                    name,
                    cluster,
                    entry_type: DirEntryType::Folder,
                })
            } else {
                dir_entries.push(DirEntry {
                    name,
                    cluster,
                    entry_type: DirEntryType::File(entry.size),
                })
            }
        }
        false
    }

    pub fn read_directory(&self, mut cluster: u32) -> Vec<DirEntry> {
        let mut entries = Vec::new();
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

    pub fn read_file(&self, mut cluster: u32, length: usize) -> Vec<u8> {
        let mut filebuffer = vec![0u8; (length + 511) & !511];

        let mut sectors_to_read = (length + 511) / 512;
        let mut buffer_offset = 0;

        while sectors_to_read > 0 {
            let read_amount = core::cmp::min(
                sectors_to_read,
                self.bios_parameter_block.sectors_per_cluster as usize,
            );

            let file_sector = self.get_start_sector_of_cluster(cluster);

            self.disk.read(
                file_sector as usize,
                read_amount as u32,
                &mut filebuffer[buffer_offset..],
            );
            sectors_to_read -= read_amount;
            buffer_offset += read_amount * 512;

            cluster = self.get_next_cluster(cluster);
        }

        filebuffer.truncate(length);

        filebuffer
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

    let fat: FAT;

    if total_clusters < 4085 {
        todo!("FAT12 Not supported yet")
    } else if total_clusters < 65535 {
        let fat16ext =
            unsafe { *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT16Ext) };
        fat = FAT {
            bios_parameter_block: bpb,
            fat_ebr: FatExtendedBootRecord::FAT16(fat16ext),
            disk,
        };
    } else {
        let fat32ext =
            unsafe { *(buffer.as_ptr().add(size_of::<BiosParameterBlock>()) as *const FAT32Ext) };
        fat = FAT {
            bios_parameter_block: bpb,
            fat_ebr: FatExtendedBootRecord::FAT32(fat32ext),
            disk,
        };
    }

    let root = fat.read_root_directory();

    // let mut tree = Vec::new();
    // tree.push(root);

    println!("/");
    tree(&fat, root, "".to_string());
}

// fn read_cluster()

fn tree(fat: &FAT, entry: Vec<DirEntry>, prefix: String) {
    let mut it = entry.into_iter().peekable();
    while let Some(e) = it.next() {
        let (t, pref) = match it.peek() {
            Some(_) => ('├', "│   "),
            None => ('└', "    "),
        };
        println!("{}{}── {}", &prefix, t, e.name);

        match e.entry_type {
            DirEntryType::Folder => {
                let nxt = fat.read_directory(e.cluster);
                tree(fat, nxt, prefix.clone() + pref);
            }
            DirEntryType::File(_len) => {
                // let file = fat.read_file(e.cluster, len as usize);
                // let mut iter = file.into_iter().array_chunks::<512>();

                // for i in iter.by_ref() {
                //     print!("{}", String::from_utf8_lossy(&i));
                // }
                // if let Some(i) = iter.into_remainder() {
                //     println!("{}", String::from_utf8_lossy(i.as_slice()));
                // }
            }
        }
    }
}
