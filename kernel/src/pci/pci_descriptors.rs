pub const DEVICE_CLASSES: &[&str] = &[
    "Unclassified",
    "Mass Storage Controller",
    "Network Controller",
    "Display Controller",
    "Multimedia Controller",
    "Memory Controller",
    "Bridge Device",
    "Simple Communication Controller",
    "Base System Peripheral",
    "Input Device Controller",
    "Docking Station",
    "Processor",
    "Serial Bus Controller",
    "Wireless Controller",
    "Intelligent Controller",
    "Satellite Communication Controller",
    "Encryption Controller",
    "Signal Processing Controller",
    "Processing Accelerator",
    "Non Essential Instrumentation",
];

// pub const fn get_subclass_name<'a>(class_code: u8, subclass_code: u8) -> Option<&'a str> {
//     match class_code {
//         0x01 => { // Mass Storage
//         }
//     }
// }
pub const fn get_vendor_name<'a>(vendor_id: u16) -> Option<&'a str> {
    match vendor_id {
        0x8086 => Some("Intel"),
        0x1022 => Some("AMD"),
        0x10DE => Some("NVIDIA"),
        _ => None,
    }
}

pub const fn get_device_name<'a>(vendor_id: u16, device_id: u16) -> Option<&'a str> {
    match vendor_id {
        0x8086 => match device_id {
            0x10D3 => Some("82574L Gigabit Network Connection"),
            0x29C0 => Some("Express DRAM Controller"),
            0x2918 => Some("LPC Interface Controller"),
            0x2922 => Some("6 port SATA Controller [AHCI mode]"),
            0x2930 => Some("SMBus Controller"),
            _ => None,
        },
        0x1022 => match device_id {
            0x2000 => Some("AMD PCNET (AM79c973)"),
            _ => None,
        },
        0x10DE => match device_id {
            _ => None,
        },
        _ => None,
    }
}
