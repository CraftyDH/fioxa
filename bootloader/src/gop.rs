use core::sync::atomic::AtomicPtr;

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

pub fn initialize_gop(bt: &uefi::table::boot::BootServices) -> &mut GraphicsOutput {
    let gop = match bt.locate_protocol::<GraphicsOutput>() {
        Ok(status) => unsafe { &mut *status.unwrap().get() },
        Err(e) => {
            error!("Cannot locate GOP: {:?}", e);
            loop {}
        }
    };

    // The max resolution to choose
    // let maxx = 1600;
    // let maxy = 1400;

    let maxx = 1920;
    let maxy = 1080;

    // let maxx = usize::MAX;
    // let maxy = usize::MAX;

    let mut best_mode = None;

    for mode in gop.modes() {
        let mode = mode.unwrap();
        let info = mode.info();
        let (x, y) = info.resolution();
        if x <= maxx && y <= maxy {
            best_mode = Some(mode)
        }
    }

    if let Some(mode) = best_mode {
        // let mode = modes.last().unwrap();
        info!("{:?}", mode.info());

        let gop2 = match bt.locate_protocol::<GraphicsOutput>() {
            Ok(status) => unsafe { &mut *status.unwrap().get() },
            Err(e) => {
                error!("Cannot locate GOP: {:?}", e);
                loop {}
            }
        };

        gop2.set_mode(&mode).unwrap().unwrap();
    }
    gop
}

pub fn get_gop_info(gop: &mut GraphicsOutput) -> GopInfo {
    let gopinfo = gop.current_mode_info();
    gopinfo.pixel_format();
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
