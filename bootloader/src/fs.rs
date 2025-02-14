use alloc::boxed::Box;
use uefi::{
    CStr16, Error,
    boot::{image_handle, locate_device_path, open_protocol_exclusive},
    proto::{
        device_path::DevicePath,
        loaded_image::LoadedImage,
        media::{
            file::{Directory, File, FileAttribute, FileInfo, FileMode, FileType},
            fs::SimpleFileSystem,
        },
    },
};

pub unsafe fn get_root_fs() -> Result<Directory, Error> {
    // Get a pointer to the load image UEFI table
    open_protocol_exclusive::<LoadedImage>(image_handle())
        // Get a pointer to the boot device (aka what disk we are currently on)
        .and_then(|loaded_image| {
            open_protocol_exclusive::<DevicePath>(loaded_image.device().unwrap())
        })
        // Find a pointer to the simple file system
        .and_then(|device_path| locate_device_path::<SimpleFileSystem>(&mut &*device_path))
        // Open the simple file system, with us as the only consumer
        .and_then(|sfs_handle| open_protocol_exclusive::<SimpleFileSystem>(sfs_handle))
        // Open the root directory aka "/"
        .and_then(|mut fs| fs.open_volume())
}

pub fn read_file<'s>(root: &mut Directory, path: &CStr16) -> Result<Box<[u8]>, &'static str> {
    // Find the file and open it
    let file = match File::open(root, path, FileMode::Read, FileAttribute::READ_ONLY) {
        Ok(file) => file,
        Err(_e) => return Err("File error..."),
    };

    // Kernal must be a file
    let mut file = match file.into_type().unwrap() {
        FileType::Regular(file) => file,
        FileType::Dir(_) => {
            return Err("The file appears to be a dir");
        }
    };

    let info = File::get_boxed_info::<FileInfo>(&mut file).unwrap();

    // Read the file
    let mut data_buffer = vec![0u8; info.file_size() as usize].into_boxed_slice();

    let bytes_read = file.read(&mut data_buffer).unwrap();

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
