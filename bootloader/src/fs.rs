use core::slice;

use uefi::{
    prelude::BootServices,
    proto::{
        device_path::DevicePath,
        loaded_image::LoadedImage,
        media::{
            file::{Directory, File, FileAttribute, FileInfo, FileMode, FileType},
            fs::SimpleFileSystem,
        },
    },
    table::boot::MemoryType,
    Handle, Status,
};

pub fn get_root_fs(boot_services: &BootServices, image_handle: Handle) -> Directory {
    let loaded_image = boot_services
        .handle_protocol::<LoadedImage>(image_handle)
        .unwrap()
        .expect("Failed to get Loaded Image from the Handle");
    let loaded_image = unsafe { &*loaded_image.get() };

    let device_path = boot_services
        .handle_protocol::<DevicePath>(loaded_image.device())
        .unwrap()
        .expect("Failed to get Device Path from image Handle");
    let device_path = unsafe { &mut *device_path.get() };

    let device_handle = boot_services
        .locate_device_path::<SimpleFileSystem>(device_path)
        .unwrap()
        .unwrap();

    let fs = boot_services
        .handle_protocol::<SimpleFileSystem>(device_handle)
        .unwrap()
        .unwrap();

    // Get access to the pointer
    let fs = unsafe { &mut *fs.get() };

    // Open the root directory aka "/"
    fs.open_volume().unwrap().unwrap()
}

pub fn read_file(
    boot_services: &BootServices,
    root: &mut Directory,
    path: &str,
) -> Result<&'static [u8], &'static str> {
    // Find the file and open it
    let file = match File::open(root, path, FileMode::Read, FileAttribute::READ_ONLY) {
        Ok(file) => file.unwrap(),
        Err(_e) => return Err("File error..."),
    };

    // Kernal must be a file
    let mut file = match file.into_type().unwrap().unwrap() {
        FileType::Regular(file) => file,
        FileType::Dir(_) => {
            return Err("The kernel appears to be a dir");
        }
    };

    // 0x1000 Bytes for the header should be suffient
    let mut info_buffer = {
        let size = 0x1000;
        let ptr = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { slice::from_raw_parts_mut(ptr, size) }
    };

    let info = match File::get_info::<FileInfo>(&mut file, &mut info_buffer) {
        Ok(file) => file.unwrap(),
        Err(e) if e.status() == Status::BUFFER_TOO_SMALL => {
            panic!("Buffer too small");
            // // Header needs a bigger buffer :(
            // let size = e.data().unwrap();
            // // Increase buffer to size requested
            // info_buffer.resize(size, 0);
            // // This time size should be right panic otherwise.
            // File::get_info::<FileInfo>(&mut file, &mut info_buffer)
            //     .expect("Incorrect size given")
            //     .unwrap()
        }
        Err(e) => {
            error!("{:?} : {:?}", e.status(), e.data());
            loop {}
        }
    };

    // Read the file
    let mut data_buffer = {
        let size = info.file_size() as usize;
        let ptr = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { slice::from_raw_parts_mut(ptr, size) }
    };

    let bytes_read = file.read(&mut data_buffer).unwrap().unwrap();

    // Check that we read all of the kernel
    if bytes_read as u64 != info.file_size() {
        warn!(
            "Only read {} bytes out of {} from file {}",
            bytes_read,
            info.file_size(),
            path
        )
    }

    return Ok(data_buffer);
}
