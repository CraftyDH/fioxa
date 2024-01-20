use modular_bitfield::{
    bitfield,
    specifiers::{B3, B4},
};

#[repr(u8)]
pub enum FISTYPE {
    REGH2D = 0x27,
    REGD2H = 0x34,
    DMAACT = 0x39,
    DMASETUP = 0x41,
    DATA = 0x46,
    BIST = 0x58,
    PIOSETUP = 0x5F,
    DEVBITS = 0xA1,
}
#[bitfield]
pub struct FisRegH2D {
    pub fis_type: u8,
    pub port_multiplier: B4,
    #[skip]
    _rsv0: B3,
    pub command_control: bool,
    pub command: u8,
    pub feature_low: u8,

    pub lba0: u8,
    pub lba1: u8,
    pub lba2: u8,
    pub device_register: u8,
    pub lba3: u8,
    pub lba4: u8,
    pub lba5: u8,
    pub feature_high: u8,

    pub countl: u8,
    pub counth: u8,
    pub iso_command_completion: u8,
    pub control: u8,
    #[skip]
    _rsv1: u32,
}

#[allow(unused)]
pub struct ReceivedFis([u8; 256]);
