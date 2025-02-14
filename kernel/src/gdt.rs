use conquer_once::spin::Lazy;
use x86_64::{
    PrivilegeLevel, VirtAddr,
    instructions::{
        segmentation::{CS, SS, Segment},
        tables::load_tss,
    },
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST_INDEX: u16 = 1;
pub const TSS_STACK_SIZE: usize = 0x1000 * 5;

// GDT Segment Selectors
pub const KERNEL_CODE_SELECTOR: SegmentSelector = SegmentSelector::new(1, PrivilegeLevel::Ring0);
pub const KERNEL_DATA_SELECTOR: SegmentSelector = SegmentSelector::new(2, PrivilegeLevel::Ring0);
// In Long Mode, userland CS will be loaded from STAR 63:48 + 16 and userland SS from STAR 63:48 + 8 on SYSRET. You may need to modify your GDT accordingly.
pub const USER_CODE_SELECTOR: SegmentSelector = SegmentSelector::new(4, PrivilegeLevel::Ring3);
pub const USER_DATA_SELECTOR: SegmentSelector = SegmentSelector::new(3, PrivilegeLevel::Ring3);
pub const TSS_SELECTOR: SegmentSelector = SegmentSelector::new(5, PrivilegeLevel::Ring0);

pub static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        static mut STACK: [u8; TSS_STACK_SIZE] = [0; TSS_STACK_SIZE];

        let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
        stack_start + TSS_STACK_SIZE as u64
    };
    tss
});

pub static BOOTGDT: Lazy<GlobalDescriptorTable> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    gdt.append(Descriptor::kernel_code_segment());
    gdt.append(Descriptor::kernel_data_segment());
    gdt.append(Descriptor::user_data_segment());
    gdt.append(Descriptor::user_code_segment());
    gdt.append(Descriptor::tss_segment(&TSS));
    gdt
});

pub unsafe fn init_bootgdt() {
    BOOTGDT.load();
    unsafe {
        CS::set_reg(KERNEL_CODE_SELECTOR);
        SS::set_reg(KERNEL_DATA_SELECTOR);
        load_tss(TSS_SELECTOR);
    }
}

pub struct CPULocalGDT {
    pub gdt: GlobalDescriptorTable,
    pub tss: TaskStateSegment,
    pub tss_stack: [[u8; TSS_STACK_SIZE]; 10],
}

impl CPULocalGDT {
    pub unsafe fn load(&'static self) {
        self.gdt.load();
        unsafe {
            CS::set_reg(KERNEL_CODE_SELECTOR);
            SS::set_reg(KERNEL_DATA_SELECTOR);
            load_tss(TSS_SELECTOR);
        }
    }
}

pub unsafe fn create_gdt_for_core(gdt: &'static mut CPULocalGDT) {
    gdt.gdt = GlobalDescriptorTable::new();
    gdt.gdt.append(Descriptor::kernel_code_segment());
    gdt.gdt.append(Descriptor::kernel_data_segment());

    gdt.gdt.append(Descriptor::user_data_segment());
    gdt.gdt.append(Descriptor::user_code_segment());

    gdt.tss = TaskStateSegment::new();

    unsafe {
        gdt.tss.privilege_stack_table[0] =
            VirtAddr::from_ptr(gdt.tss_stack[0].as_ptr().add(TSS_STACK_SIZE));

        gdt.tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::from_ptr(gdt.tss_stack[1].as_ptr().add(TSS_STACK_SIZE));

        gdt.tss.interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] =
            VirtAddr::from_ptr(gdt.tss_stack[2].as_ptr().add(TSS_STACK_SIZE));
    }

    gdt.gdt.append(Descriptor::tss_segment(&gdt.tss));
}
