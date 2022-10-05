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
    CStr16, Error, Handle, Status,
};

use crate::{get_buffer, OwnedBuffer};

pub unsafe fn get_root_fs(
    boot_services: &BootServices,
    image_handle: Handle,
) -> Result<Directory, Error> {
    boot_services
        // Get a pointer to the load image UEFI table
        .open_protocol_exclusive::<LoadedImage>(image_handle)
        // Get a pointer to the boot device (aka what disk we are currently on)
        .and_then(|loaded_image| {
            boot_services.open_protocol_exclusive::<DevicePath>(loaded_image.device())
        })
        // Find a pointer to the simple file system
        .and_then(|device_path| {
            boot_services.locate_device_path::<SimpleFileSystem>(&mut &*device_path)
        })
        // Open the simple file system, with us as the only consumer
        .and_then(|sfs_handle| {
            boot_services.open_protocol_exclusive::<SimpleFileSystem>(sfs_handle)
        })
        // Open the root directory aka "/"
        .and_then(|mut fs| fs.open_volume())
}

pub fn read_file<'b, 's>(
    boot_services: &'b BootServices,
    root: &mut Directory,
    path: &CStr16,
) -> Result<OwnedBuffer<'b, 's>, &'static str> {
    let buf = unsafe { read_file_no_drop(boot_services, root, path)? };
    Ok(OwnedBuffer::from_buf(boot_services, buf))
}

/// Unsafe because it doesn't drop the buffer
pub unsafe fn read_file_no_drop<'b, 's>(
    boot_services: &'b BootServices,
    root: &mut Directory,
    path: &CStr16,
) -> Result<&'s mut [u8], &'static str> {
    // Find the file and open it
    let file = match File::open(root, path, FileMode::Read, FileAttribute::READ_ONLY) {
        Ok(file) => file,
        Err(_e) => return Err("File error..."),
    };

    // Kernal must be a file
    let mut file = match file.into_type().unwrap() {
        FileType::Regular(file) => file,
        FileType::Dir(_) => {
            return Err("The kernel appears to be a dir");
        }
    };

    // 0x1000 Bytes for the header should be suffient
    let mut info_buffer = OwnedBuffer::new(boot_services, 0x1000);

    let info = match File::get_info::<FileInfo>(&mut file, &mut info_buffer.buf) {
        Ok(file) => file,
        Err(e) if e.status() == Status::BUFFER_TOO_SMALL => {
            panic!("File header buffer too small");
        }
        Err(e) => {
            error!("{:?} : {:?}", e.status(), e.data());
            loop {}
        }
    };

    // Read the file
    let data_buffer = unsafe { get_buffer(boot_services, info.file_size() as usize) };

    let bytes_read = file.read(data_buffer).unwrap();

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
