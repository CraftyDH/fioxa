use alloc::sync::Arc;
use fioxa_rpc::{fs_capnp, server::RPCServer, service::ServiceExecutor};
use hashbrown::HashMap;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{channel::Channel, handle::Handle};

#[rustfmt::skip]
pub const LOOKUP: &[(&str, &[u8])] = &[
    ("font.psf",    include_bytes!("../../builder/assets/zap-light16.psf")),
    ("terminal",    include_bytes!("../../builder/fioxa/apps/terminal")),
    ("amd_pcnet",   include_bytes!("../../builder/fioxa/drivers/amd_pcnet")),
    ("ps2",         include_bytes!("../../builder/fioxa/drivers/ps2")),
    ("fat",         include_bytes!("../../builder/fioxa/drivers/fat")),
];

pub fn early_bootfs_get(file: &str) -> Option<&'static [u8]> {
    for (name, entry) in LOOKUP.iter().copied() {
        if file == name {
            return Some(entry);
        }
    }
    None
}

pub fn serve_bootfs() {
    let mut hash = HashMap::new();

    for e in LOOKUP {
        let (l, r) = Channel::new();
        hash.insert(e.0, Arc::new(l.into_inner()));
        sys_process_spawn_thread(move || {
            ServiceExecutor::from_channel(r, |chan| RPCServer::new(chan, BootFsFile(e.1)).run())
                .run()
                .unwrap()
        });
    }

    let map = Arc::new(hash);
    ServiceExecutor::with_name("FS", |c| {
        let map = map.clone();
        sys_process_spawn_thread(move || RPCServer::new(c, BootFsRoot(map)).run());
    })
    .run()
    .unwrap();
    panic!("bootfs exited")
}

struct BootFsRoot(Arc<HashMap<&'static str, Arc<Handle>>>);

impl fioxa_rpc::fs::FolderService for BootFsRoot {
    fn get_children<'a>(
        &mut self,
        _req: fioxa_rpc::OwnedReader<'a, fs_capnp::folder_get_children::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::folder_got_children::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let mut res = res.init_entries(LOOKUP.len().try_into().unwrap());
        for (i, e) in LOOKUP.iter().enumerate() {
            res.reborrow().get(i.try_into().unwrap()).set_name(e.0);
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

        let entry = self.0.get(name);

        match entry {
            Some(e) => {
                res.set_type(fs_capnp::FileType::File);
                res_handles.add(res.init_capability(), e.clone());
            }
            None => {
                res.set_type(fs_capnp::FileType::None);
            }
        }

        Ok(())
    }
}

struct BootFsFile(&'static [u8]);

impl fioxa_rpc::fs::FileService for BootFsFile {
    fn size<'a>(
        &mut self,
        _req: fioxa_rpc::OwnedReader<'a, fs_capnp::file_size::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::file_size_read::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        res.set_size(self.0.len().try_into().unwrap());
        Ok(())
    }

    fn read<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, fs_capnp::file_read::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, fs_capnp::file_data::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let offset = req.get_offset() as usize;
        if offset > self.0.len() {
            return Ok(());
        }

        let data = &self.0[offset..(offset + req.get_len() as usize).min(self.0.len())];

        res.set_data(data);

        Ok(())
    }
}
