use core::slice;

use types::{PSF1Font, PSF1FontHeader, PSF1_MAGIC};
use uefi::{
    prelude::BootServices,
    proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode, FileType},
    table::boot::MemoryType,
    Status,
};

pub fn load_psf1_font(boot_services: &BootServices, root: &mut Directory, path: &str) -> PSF1Font {
    // Find the font and open it
    let psf1 = match File::open(root, path, FileMode::Read, FileAttribute::READ_ONLY) {
        Ok(psf1) => psf1.unwrap(),
        Err(e) => {
            info!("Cant find {:?}", e);
            loop {}
        }
    };

    // Font must be a file
    let mut psf1 = match psf1.into_type().unwrap().expect("Failed to get psf1 font") {
        FileType::Regular(file) => file,
        FileType::Dir(_) => {
            info!("psf1 is a dir ???");
            loop {}
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

    let info = match File::get_info::<FileInfo>(&mut psf1, &mut info_buffer) {
        Ok(file) => file.unwrap(),
        Err(e) if e.status() == Status::BUFFER_TOO_SMALL => {
            panic!("Buffer too small");
        }
        Err(e) => {
            error!("{:?} : {:?}", e.status(), e.data());
            loop {}
        }
    };

    let mut psf1_font_header_buffer = {
        let size = core::mem::size_of::<PSF1FontHeader>();
        let ptr = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { slice::from_raw_parts_mut(ptr, size) }
    };

    // let mut psf1_font = vec![0; core::mem::size_of::<PSF1FontHeader>()];
    let _bytes_read = psf1.read(&mut psf1_font_header_buffer).unwrap().unwrap();

    let psf1_font_header = unsafe { psf1_font_header_buffer.align_to::<PSF1FontHeader>().1[0] };

    if psf1_font_header.magic != PSF1_MAGIC {
        error!("PSF1 FONT not valid");
        loop {}
    }

    let mut glyph_buffer_size = (psf1_font_header.charsize as usize) * 256;
    if psf1_font_header.mode_512 == 1 {
        // 512 glyph mode
        glyph_buffer_size *= 2;
    }

    let mut psf1_font = {
        let size = glyph_buffer_size;
        let ptr = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { slice::from_raw_parts_mut(ptr, size) }
    };

    let _bytes_read = psf1.read(&mut psf1_font).unwrap().unwrap();

    let mut unicode_table_buffer = {
        let size =
            info.file_size() as usize - glyph_buffer_size - core::mem::size_of::<PSF1FontHeader>();
        let ptr = boot_services
            .allocate_pool(MemoryType::LOADER_DATA, size)
            .unwrap()
            .unwrap();
        unsafe { slice::from_raw_parts_mut(ptr, size) }
    };

    let _bytes_read = psf1.read(&mut unicode_table_buffer).unwrap().unwrap();

    return PSF1Font {
        psf1_header: psf1_font_header,
        glyph_buffer: psf1_font,
        unicode_buffer: unicode_table_buffer,
    };
}
