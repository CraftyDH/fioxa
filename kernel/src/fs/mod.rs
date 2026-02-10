pub mod fat;
pub mod mbr;

use alloc::{sync::Arc, vec::Vec};
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_userspace::{
    channel::Channel,
    disk::{DiskControllerExecutor, DiskControllerImpl, DiskControllerService, DiskService},
    fs::{FSControllerExecutor, FSControllerImpl},
    ipc::IPCChannel,
    mutex::Mutex,
    service::ServiceExecutor,
};

use crate::fs::mbr::read_partitions;

pub struct FSPartitionDisk {
    backing_disk: Arc<Mutex<DiskService>>,
    partition_offset: u64,
    partition_length: u64,
}

impl FSPartitionDisk {
    pub fn new(
        backing_disk: Arc<Mutex<DiskService>>,
        partition_offset: u64,
        partition_length: u64,
    ) -> Self {
        Self {
            backing_disk,
            partition_offset,
            partition_length,
        }
    }

    fn read(&self, sector: u64, sector_count: u64) -> Vec<u8> {
        assert!(sector + sector_count <= self.partition_length);
        self.backing_disk
            .lock()
            .read(sector + self.partition_offset, sector_count)
            .deserialize()
            .unwrap()
    }
}

pub fn file_system_partition_loader() {
    let mut controller =
        DiskControllerService::from_channel(IPCChannel::connect("DISK_CONTROLLER"));

    for disk in controller.get_disks(true) {
        // TODO: Work out why some disks don't answer the request
        // info!("{:?}", disk.identify());
        let disk = DiskService::from_channel(IPCChannel::from_channel(disk.connect().unwrap()));
        read_partitions(Arc::new(Mutex::new(disk)));
    }

    panic!("the iterator should never end")
}

pub fn disk_controller() {
    let data = Arc::new(Mutex::new(DiskControllerData::new()));
    ServiceExecutor::with_name("DISK_CONTROLLER", |chan| {
        let data = data.clone();
        sys_process_spawn_thread(|| {
            match DiskControllerExecutor::new(
                IPCChannel::from_channel(chan),
                DiskControllerHandler { common: data },
            )
            .run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
}

pub fn fs_controller() {
    let data = Arc::new(Mutex::new(FSControllerData::new()));
    ServiceExecutor::with_name("FS_CONTROLLER", |chan| {
        let data = data.clone();
        sys_process_spawn_thread(|| {
            match FSControllerExecutor::new(
                IPCChannel::from_channel(chan),
                FSControllerHandler { common: data },
            )
            .run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
}

struct DiskControllerData {
    disks: Vec<Channel>,
    waiters: Vec<IPCChannel>,
}

impl DiskControllerData {
    pub fn new() -> Self {
        Self {
            disks: Vec::new(),
            waiters: Vec::new(),
        }
    }
}

struct DiskControllerHandler {
    common: Arc<Mutex<DiskControllerData>>,
}

impl DiskControllerImpl for DiskControllerHandler {
    fn register_disk(&mut self, chan: Channel) {
        let mut common = self.common.lock();
        for w in common.waiters.iter_mut() {
            match w.send(&chan) {
                Ok(()) => (),
                Err(e) => {
                    warn!("error sending {e}");
                    return;
                }
            }
        }
        common.disks.push(chan);
    }

    fn get_disks(&mut self, updates: bool) -> Channel {
        let mut common = self.common.lock();

        let (send, res) = Channel::new();
        let mut send = IPCChannel::from_channel(send);

        for disk in common.disks.iter_mut() {
            match send.send(disk) {
                Ok(()) => (),
                Err(e) => {
                    warn!("error sending {e}");
                    return res;
                }
            }
        }

        if updates {
            common.waiters.push(send);
        }
        res
    }
}

struct FSControllerData {
    disks: Vec<Channel>,
    waiters: Vec<IPCChannel>,
}

impl FSControllerData {
    pub fn new() -> Self {
        Self {
            disks: Vec::new(),
            waiters: Vec::new(),
        }
    }
}

struct FSControllerHandler {
    common: Arc<Mutex<FSControllerData>>,
}

impl FSControllerImpl for FSControllerHandler {
    fn register_filesystem(&mut self, chan: Channel) {
        let mut common = self.common.lock();
        for w in common.waiters.iter_mut() {
            match w.send(&chan) {
                Ok(()) => (),
                Err(e) => {
                    warn!("error sending {e}");
                    return;
                }
            }
        }
        common.disks.push(chan);
    }

    fn get_filesystems(&mut self, updates: bool) -> Channel {
        let mut common = self.common.lock();

        let (send, res) = Channel::new();
        let mut send = IPCChannel::from_channel(send);

        for disk in common.disks.iter() {
            match send.send(disk) {
                Ok(()) => (),
                Err(e) => {
                    warn!("error sending {e}");
                    return res;
                }
            }
        }

        if updates {
            common.waiters.push(send);
        }
        res
    }
}
