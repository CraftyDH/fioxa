pub mod tss;

use lazy_static::lazy_static;
use x86_64::{
    instructions::{
        segmentation::{Segment, CS, SS},
        tables::load_tss,
    },
    structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
};

pub struct Selectors {
    pub code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    pub static ref GDT: [(GlobalDescriptorTable, Selectors); 8] = {
        core::array::from_fn(|i| {
            let mut gdt = GlobalDescriptorTable::new();
            let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
            let data_selector = gdt.add_entry(Descriptor::kernel_data_segment());

            let tss_selector = gdt.add_entry(Descriptor::tss_segment(tss::TSS.get(i).unwrap()));
            (
                gdt,
                Selectors {
                    code_selector,
                    data_selector,
                    tss_selector,
                },
            )
        })
    };
}

pub fn init(core_id: usize) {
    let gdt = GDT.get(core_id).unwrap();
    gdt.0.load();

    unsafe {
        CS::set_reg(gdt.1.code_selector);
        SS::set_reg(gdt.1.data_selector);
        load_tss(gdt.1.tss_selector);
    }
}
