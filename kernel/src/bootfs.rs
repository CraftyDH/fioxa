use alloc::string::ToString;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    fs::{FSControllerService, FSFile, FSFileId, FSServiceExecutor, FSServiceImpl},
    ipc::IPCChannel,
    service::ServiceExecutor,
};

#[rustfmt::skip]
pub const LOOKUP: &[(&str, &[u8])] = &[
    ("font.psf",    include_bytes!("../../builder/assets/zap-light16.psf")),
    ("terminal",    include_bytes!("../../builder/fioxa/apps/terminal")),
    ("amd_pcnet",   include_bytes!("../../builder/fioxa/drivers/amd_pcnet")),
    ("ps2",         include_bytes!("../../builder/fioxa/drivers/ps2")),
];

pub fn early_bootfs_get(file: &str) -> Option<&'static [u8]> {
    for (name, entry) in LOOKUP.iter().copied() {
        if file == name {
            return Some(entry);
        }
    }
    None
}

pub const ROOT: FSFileId = FSFileId(0);

pub fn serve_bootfs() {
    let (chan, client) = Channel::new();
    {
        let mut fs_controller =
            FSControllerService::from_channel(IPCChannel::connect("FS_CONTROLLER"));
        fs_controller.register_filesystem(client);
    }

    ServiceExecutor::from_channel(chan, |c| {
        sys_process_spawn_thread(move || {
            FSServiceExecutor::new(IPCChannel::from_channel(c), BootFs)
                .run()
                .unwrap();
        });
    })
    .run()
    .unwrap();
}

struct BootFs;

impl FSServiceImpl for BootFs {
    fn stat_root(&mut self) -> FSFile {
        FSFile {
            id: ROOT,
            file: kernel_userspace::fs::FSFileType::Folder,
        }
    }

    fn stat_by_id(&mut self, file: FSFileId) -> Option<FSFile> {
        if file == ROOT {
            return Some(self.stat_root());
        }
        for (inode, entry) in (1u64..).zip(LOOKUP) {
            if inode == file.0 {
                return Some(FSFile {
                    id: file,
                    file: kernel_userspace::fs::FSFileType::File {
                        length: entry.1.len(),
                    },
                });
            }
        }
        None
    }

    fn get_children(
        &mut self,
        file: FSFileId,
    ) -> Option<hashbrown::HashMap<alloc::string::String, FSFileId>> {
        if file == ROOT {
            let map = (1u64..)
                .zip(LOOKUP)
                .map(|(inode, (name, _))| (name.to_string(), FSFileId(inode)))
                .collect();
            Some(map)
        } else {
            None
        }
    }

    fn read_file(
        &mut self,
        file: FSFileId,
        offset: usize,
        len: usize,
    ) -> Option<alloc::vec::Vec<u8>> {
        for (inode, entry) in (1u64..).zip(LOOKUP) {
            if inode == file.0 {
                if offset > entry.1.len() {
                    return Some(vec![]);
                }
                return Some(entry.1[offset..(offset + len).min(entry.1.len())].to_vec());
            }
        }
        None
    }
}
