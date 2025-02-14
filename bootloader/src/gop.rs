use core::sync::atomic::AtomicPtr;

use uefi::boot::ScopedProtocol;
use uefi::boot::get_handle_for_protocol;
use uefi::boot::open_protocol_exclusive;
use uefi::proto::console::gop::GraphicsOutput;

use uefi::proto::console::gop::PixelFormat;

#[derive(Debug)]
pub struct GopInfo {
    pub buffer: AtomicPtr<u8>,
    pub buffer_size: usize,
    pub horizonal: usize,
    pub vertical: usize,
    pub stride: usize,
    pub pixel_format: PixelFormat,
}

pub fn initialize_gop() -> ScopedProtocol<GraphicsOutput> {
    let mut gop = get_handle_for_protocol::<GraphicsOutput>()
        .and_then(|handle| open_protocol_exclusive::<GraphicsOutput>(handle))
        .unwrap();

    // The max resolution to choose
    // let maxx = 1600;
    // let maxy = 1400;

    let maxx = 1920;
    let maxy = 1080;

    // let maxx = usize::MAX;
    // let maxy = usize::MAX;

    let mut best_mode = None;

    for mode in gop.modes() {
        let mode = mode;
        let info = mode.info();
        let (x, y) = info.resolution();
        if x <= maxx && y <= maxy {
            best_mode = Some(mode)
        }
    }

    if let Some(mode) = &best_mode {
        info!("Choosing GOP mode: {:?}", mode.info());

        gop.set_mode(&mode).unwrap();
    }

    gop
}

pub fn get_gop_info(gop: &mut GraphicsOutput) -> GopInfo {
    let gopinfo = gop.current_mode_info();
    let mut gopbuf = gop.frame_buffer();
    let (horizonal, vertical) = gopinfo.resolution();

    info!("Loc: {:?}", gopbuf.as_mut_ptr());

    GopInfo {
        buffer: AtomicPtr::from(gopbuf.as_mut_ptr()),
        buffer_size: gopbuf.size(),
        horizonal,
        vertical,
        stride: gopinfo.stride(),
        pixel_format: gopinfo.pixel_format(),
    }
}
