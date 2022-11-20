#[repr(C, align(2))]
#[derive(Debug)]
pub struct ATADiskIdentify {
    pub config: u16,
    pub cylinders: u16,
    pub specconf: u16,
    pub heads: u16,

    pub _obsolete2: [u16; 2],

    pub sectors: u16,
    pub vendor: [u16; 3],
    pub serial: [u8; 20],

    pub _retired20: [u16; 2],
    pub _obsolete22: u16,

    pub firmware_revision: [u8; 8],
    pub model: [u8; 40],
    pub sectors_per_interrupt: u16,
    pub tcg: u16, /* Trusted Computing Group */

    pub capabilities1: u16,
    pub capabilities2: u16,

    pub _retired_piomode: u16,
    pub _retired_dmamode: u16,

    pub ata_valid: u16,

    pub current_cylinders: u16,
    pub current_heads: u16,
    pub current_sectors: u16,
    pub current_size_1: u16,
    pub current_size_2: u16,
    pub multi: u16,

    pub lba_size_1: u16,
    pub lba_size_2: u16,
    pub _obsolete62: u16,

    pub multiword_dma_modes: u16,
    pub apio_modes: u16,

    pub mwdmamin: u16,
    pub mwdmarec: u16,
    pub pioblind: u16,
    pub pioiordy: u16,
    pub support3: u16,

    pub _reserved70: u16,
    pub rlsovlap: u16,
    pub rlsservice: u16,
    pub _reserved73: u16,
    pub _reserved74: u16,
    pub queue: u16,

    pub sata_capabilities: u16,
    pub sata_capabilities2: u16,
    pub sata_support: u16,
    pub sata_enabled: u16,
    pub version_major: u16,
    pub version_minor: u16,

    pub command_1: u16,
    pub command2: u16,
    pub extension: u16,

    pub ultra_dma_modes: u16,
    pub erase_time: u16,
    pub enhanced_erase_time: u16,
    pub apm_value: u16,
    pub master_passwd_revision: u16,
    pub hwres: u16,

    pub acoustic: u16,

    pub stream_min_req_size: u16,
    pub stream_transfer_time: u16,
    pub stream_access_latency: u16,
    pub stream_granularity: u32,
    pub lba_size48_1: u16,
    pub lba_size48_2: u16,
    pub lba_size48_3: u16,
    pub lba_size48_4: u16,
    pub _reserved104: u16,

    pub max_dsm_blocks: u16,
    pub pss: u16,

    pub isd: u16,
    pub wwm: [u16; 4],
    pub _reserved112: [u16; 5],
    pub lss_1: u16,
    pub lss_2: u16,
    pub support2: u16,

    pub enabled2: u16,
    pub _reserved121: [u16; 6],
    pub removable_status: u16,
    pub security_status: u16,

    pub _reserved129: [u16; 31],
    pub cfa_powermode1: u16,
    pub _reserved161: u16,
    pub cfa_kms_support: u16,
    pub cfa_trueide_modes: u16,
    pub cfa_memory_modes: u16,
    pub _reserved165: [u16; 3],
    pub form_factor: u16,

    pub support_dsm: u16,

    pub product_id: [u8; 8],
    pub _reserved174: [u16; 2],
    pub media_serial: [u8; 60],
    pub sct: u16,
    pub _reserved207: [u16; 2],
    pub lsalign: u16,

    pub wrv_sectors_m3_1: u16,
    pub wrv_sectors_m3_2: u16,
    pub wrv_sectors_m2_1: u16,
    pub wrv_sectors_m2_2: u16,

    pub nv_cache_caps: u16,
    pub nv_cache_size_1: u16,
    pub nv_cache_size_2: u16,
    pub media_rotation_rate: u16,

    pub _reserved218: u16,
    pub nv_cache_opt: u16,
    pub wrv_mode: u16,
    pub _reserved221: u16,

    pub transport_major: u16,
    pub transport_minor: u16,
    pub _reserved224: [u16; 31],
    pub integrity: u16,
}
