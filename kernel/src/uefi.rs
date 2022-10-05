use uefi::{table::cfg::ConfigTableEntry, Guid};

pub fn get_config_table(guid: Guid, entries: &[ConfigTableEntry]) -> Option<&ConfigTableEntry> {
    for elem in entries {
        if elem.guid == guid {
            return Some(elem);
        }
    }
    return None;
}
