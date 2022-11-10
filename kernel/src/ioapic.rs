use core::{
    mem,
    ptr::{read_volatile, write_volatile},
};

use acpi::{sdt::SdtHeader, AcpiTable};
use alloc::vec::Vec;

use crate::{
    interrupts::set_irq_handler,
    paging::page_table_manager::PageTableManager,
    ps2::{keyboard, mouse},
};

pub fn enable_apic(madt: &Madt, mapper: &mut PageTableManager) {
    let (_, _, io_apics, apic_ints) = madt.find_ioapic();

    unsafe {
        // Make our gs 0 as other cores will use their id
        core::arch::asm!("mov {0}, 0", "mov gs, {0}", out(reg) _);
    }

    for apic in &io_apics {
        println!("APIC: {:?}", apic);
        mapper
            .map_memory(apic.apic_addr.into(), apic.apic_addr.into(), true)
            .unwrap()
            .flush();
    }

    let apic = io_apics.first().unwrap();

    for i in apic_ints {
        println!("Int override: {:?}", i);
    }

    // Timer is usually overridden to irq 2
    // TODO: Parse overides and use those
    // 0xFF all cores
    set_redirect_entry(apic.apic_addr, 0xFF, 2, 49);

    set_irq_handler(50, keyboard::keyboard_int_handler);
    set_redirect_entry(apic.apic_addr, 0, 1, 50);

    set_irq_handler(51, mouse::mouse_int_handler);
    set_redirect_entry(apic.apic_addr, 0, 12, 51);
}

pub fn send_ipi_to(apic_id: u8, vector: u8) {
    // Check no IPI pending
    while unsafe { *((0xfee00000u64 + 0x300) as *const u32) & (1 << 12) > 0 } {}
    // Target
    unsafe { *((0xfee00000u64 + 0x310) as *mut u32) = (apic_id as u32) << 24 };
    // Send interrupt
    unsafe { *((0xfee00000u64 + 0x300) as *mut u32) = vector as u32 | 1 << 14 };
}

fn set_redirect_entry(apic_base: u32, processor: u32, irq: u8, vector: u8) {
    let mut low = read_ioapic_register(apic_base, 0x10 + 2 * irq);
    let mut high = read_ioapic_register(apic_base, 0x11 + 2 * irq);

    high &= !0xff000000;
    high |= processor << 24;
    write_ioapic_register(apic_base, 0x11 + 2 * irq, high);

    // Unmask
    low &= !(1 << 16);
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

fn write_ioapic_register(apic_base: u32, offset: u8, val: u32) {
    unsafe {
        write_volatile(apic_base as *mut u32, offset as u32);
        write_volatile((apic_base + 0x10) as *mut u32, val as u32);
    }
}

fn read_ioapic_register(apic_base: u32, offset: u8) -> u32 {
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

impl AcpiTable for Madt {
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

            match entry {
                0 => {
                    let ptr2 = unsafe { *start_ptr.add(2) };
                    let ptr3 = unsafe { *start_ptr.add(3) };
                    let ptr4 = unsafe { *start_ptr.add(4) };
                    println!("Core: {ptr2} {ptr3}");
                    if ptr4 & 1 > 0 && ptr3 <= 8 {
                        lapic_ids.push(ptr3)
                    }
                }
                _ => {}
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
            println!("E type: {entry}, {} bytes", len);

            match entry {
                0 => {
                    let ptr2 = unsafe { *start_ptr.add(2) };
                    let ptr3 = unsafe { *start_ptr.add(3) };
                    let ptr4 = unsafe { *start_ptr.add(4) };
                    println!("Core: {ptr2} {ptr3}");
                    if ptr4 & 1 > 0 && ptr3 <= 8 {
                        lapic_ids.push(ptr3)
                    }
                }
                1 => {
                    io_apics.push(unsafe { *(start_ptr.add(2) as *const IOApic) });
                }
                2 => {
                    let x = unsafe { *(start_ptr.add(2) as *const ApicInterruptOveride) };
                    println!("X{:?}", x);
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
