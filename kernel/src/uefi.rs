use bootloader::uefi::{Guid, table::cfg::ConfigTableEntry};

pub fn get_config_table(guid: Guid, entries: &[ConfigTableEntry]) -> Option<&ConfigTableEntry> {
    entries.iter().find(|&elem| elem.guid == guid)
}
