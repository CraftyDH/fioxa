use core::{
    mem,
    ptr::{read_volatile, write_volatile},
};

use acpi::{AcpiTable, sdt::SdtHeader};
use alloc::{sync::Arc, vec::Vec};
use bit_field::BitField;
use kernel_sys::types::VMMapFlags;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    interrupts::{
        com1_int_handler, keyboard_int_handler, mouse_int_handler, pci_int_handler, set_irq_handler,
    },
    lapic::LAPIC_ADDR,
    mutex::Spinlock,
    vm::VMO,
};

pub unsafe fn enable_apic(madt: &Madt) {
    let (_, _, io_apics, apic_ints) = madt.find_ioapic();

    for apic in &io_apics {
        debug!("APIC: {apic:?}");
    }

    for i in apic_ints {
        debug!("Int override: {i:?}");
    }

    let curr_proc = unsafe { CPULocalStorageRW::get_current_task().process() };
    let mut mem = curr_proc.memory.lock();

    let apic = io_apics.first().unwrap();
    let apic_mapping = unsafe {
        Arc::new(Spinlock::new(VMO::new_mmap(
            apic.apic_addr as usize,
            0x1000,
        )))
    };

    let apic_addr = mem
        .region
        .map_vmo(apic_mapping, VMMapFlags::WRITEABLE, None)
        .unwrap();

    // Timer is usually overridden to irq 2
    // TODO: Parse overides and use those
    // 0xFF all cores
    // set_redirect_entry(apic.apic_addr, 0xFF, 2, 49, true);

    set_irq_handler(50, keyboard_int_handler);
    set_redirect_entry(apic_addr, 0, 1, 50, true);

    set_irq_handler(51, mouse_int_handler);
    set_redirect_entry(apic_addr, 0, 12, 51, true);

    set_irq_handler(52, pci_int_handler);
    set_redirect_entry(apic_addr, 0, 10, 52, true);
    set_redirect_entry(apic_addr, 0, 11, 52, true);

    set_irq_handler(53, com1_int_handler);
    set_redirect_entry(apic_addr, 0, 4, 53, true);

    unsafe { mem.region.unmap(apic_addr, 0x1000).unwrap() }
}

pub fn send_ipi_to(apic_id: u8, vector: u8) {
    // Check no IPI pending
    while unsafe { read_volatile((LAPIC_ADDR + 0x300) as *const u32) & (1 << 12) > 0 } {}
    // Target
    unsafe { write_volatile((LAPIC_ADDR + 0x310) as *mut u32, (apic_id as u32) << 24) };
    // Send interrupt
    unsafe { write_volatile((LAPIC_ADDR + 0x300) as *mut u32, vector as u32 | 1 << 14) };
}

fn set_redirect_entry(apic_base: usize, processor: u32, irq: u8, vector: u8, enable: bool) {
    let mut low = read_ioapic_register(apic_base, 0x10 + 2 * irq);
    let mut high = read_ioapic_register(apic_base, 0x11 + 2 * irq);

    high &= !0xff000000;
    high |= processor << 24;
    write_ioapic_register(apic_base, 0x11 + 2 * irq, high);

    // Unmask?
    low.set_bit(16, !enable);
    // Level sensitive
    // low |= 1 << 15;

    // set to phys delivery
    low &= !(1 << 11);
    // set to fixed delivery
    low &= !(0x700);

    // Set delivery vector
    low &= !0xFF;
    low |= vector as u32;
    write_ioapic_register(apic_base, 0x10 + 2 * irq, low);
}

fn write_ioapic_register(apic_base: usize, offset: u8, val: u32) {
    unsafe {
        write_volatile(apic_base as *mut u32, offset as u32);
        write_volatile((apic_base + 0x10) as *mut u32, val);
    }
}

fn read_ioapic_register(apic_base: usize, offset: u8) -> u32 {
    unsafe {
        write_volatile(apic_base as *mut u32, offset as u32);
        read_volatile((apic_base + 0x10) as *mut u32)
    }
}

#[repr(C, packed)]
pub struct Madt {
    header: SdtHeader,
    local_apic_address: u32,
    flags: u32,
}

unsafe impl AcpiTable for Madt {
    const SIGNATURE: acpi::sdt::Signature = acpi::sdt::Signature::MADT;

    fn header(&self) -> &acpi::sdt::SdtHeader {
        &self.header
    }
}

impl Madt {
    pub fn get_lapid_ids(&self) -> Vec<u8> {
        let mut start_ptr = self as *const Madt as *mut u8;

        start_ptr = unsafe { start_ptr.add(mem::size_of::<Madt>()) };
        let mut length_left = self.header.length - mem::size_of::<Madt>() as u32;

        let mut lapic_ids: Vec<u8> = Vec::new();

        while length_left > 0 {
            let entry = unsafe { *start_ptr };
            let len = unsafe { *start_ptr.add(1) };

            if entry == 0 {
                let ptr3 = unsafe { *start_ptr.add(3) };
                let ptr4 = unsafe { *start_ptr.add(4) };
                if ptr4 & 1 > 0 && ptr3 <= 8 {
                    lapic_ids.push(ptr3)
                }
            }

            start_ptr = unsafe { start_ptr.add(len as usize) };
            length_left -= len as u32;
        }
        lapic_ids
    }

    pub fn find_ioapic(&self) -> (u64, Vec<u8>, Vec<IOApic>, Vec<ApicInterruptOveride>) {
        let mut start_ptr = self as *const Madt as *mut u8;

        start_ptr = unsafe { start_ptr.add(mem::size_of::<Madt>()) };
        let mut length_left = self.header.length - mem::size_of::<Madt>() as u32;

        let mut lapic_ids: Vec<u8> = Vec::new();
        let mut io_apics: Vec<IOApic> = Vec::new();
        let mut apic_overide: Vec<ApicInterruptOveride> = Vec::new();
        let mut lapic_addr = self.local_apic_address as u64;

        while length_left > 0 {
            let entry = unsafe { *start_ptr };
            let len = unsafe { *start_ptr.add(1) };
            // println!("E type: {entry}, {} bytes", len);

            match entry {
                0 => {
                    let ptr2 = unsafe { *start_ptr.add(2) };
                    let ptr3 = unsafe { *start_ptr.add(3) };
                    let ptr4 = unsafe { *start_ptr.add(4) };
                    trace!("Core: {ptr2} {ptr3}");
                    if ptr4 & 1 > 0 && ptr3 <= 8 {
                        lapic_ids.push(ptr3)
                    }
                }
                1 => {
                    io_apics.push(unsafe { *(start_ptr.add(2) as *const IOApic) });
                }
                2 => {
                    let x = unsafe { *(start_ptr.add(2) as *const ApicInterruptOveride) };
                    trace!("X{x:?}");
                    apic_overide.push(x);
                }
                5 => {
                    lapic_addr = unsafe { *(start_ptr.add(4) as *const u64) };
                }
                _ => (),
            }

            start_ptr = unsafe { start_ptr.add(len as usize) };
            length_left -= len as u32;
        }
        (lapic_addr, lapic_ids, io_apics, apic_overide)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IOApic {
    apic_id: u8,
    _rsv: u8,
    apic_addr: u32,
    interrupt_base: u32,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct ApicInterruptOveride {
    bus_source: u8,
    irq_source: u8,
    interrupt_num: u32,
    flags: u16,
}
