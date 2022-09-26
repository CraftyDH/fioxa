use core::cmp::{max, min};

use uefi::{
    prelude::BootServices,
    table::boot::{AllocateType, MemoryType},
};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

// For the ELF Header https://refspecs.linuxfoundation.org/elf/gabi4+/ch4.eheader.html
const ELFCLASS64: u8 = 2; // 64 BIT
const ELFDATA2LSB: u8 = 1; // LSB not MSB

const ET_EXEC: u16 = 2; // Executable file
const EM_X86_64: u16 = 62; // AMD x86-64 architecture

// For the ELF Program Header https://refspecs.linuxbase.org/elf/gabi4+/ch5.pheader.html
const PT_LOAD: u32 = 1; // A loadable segment

pub fn load_kernel(boot_services: &BootServices, kernel_data: &[u8]) -> u64 {
    // Transpose the header as an elf header
    let elf_header = unsafe { *(kernel_data.as_ptr() as *const Elf64Ehdr) };
    // Ensure that all the header flags are suitable
    if &elf_header.e_ident[0..6]
        == [
            0x7F,
            'E' as u8,
            'L' as u8,
            'F' as u8,
            ELFCLASS64,
            ELFDATA2LSB,
        ]
        && elf_header.e_type == ET_EXEC
        && elf_header.e_machine == EM_X86_64
        && elf_header.e_version == 1
    {
        info!("Kernel Header Verified");
    } else {
        panic!("Kernel Header Invalid")
    }

    let headers = (elf_header.e_phoff..((elf_header.e_phnum * elf_header.e_phentsize).into()))
        .step_by(elf_header.e_phentsize.into());

    let mut base = u64::MAX;
    let mut size = u64::MIN;

    for program_header_ptr in headers.clone() {
        let program_header = unsafe {
            *(kernel_data.as_ptr().offset(program_header_ptr as isize) as *const Elf64Phdr)
        };
        base = min(base, program_header.p_vaddr);
        size = max(size, program_header.p_vaddr + program_header.p_memsz);
    }

    let mem_start = (base / 0x1000) * 0x1000;
    // The size from start to finish
    let size = size - base;
    let pages = size / 4096 + 1;

    let _ = match boot_services.allocate_pages(
        AllocateType::Address(mem_start.try_into().unwrap()),
        MemoryType::LOADER_DATA,
        pages as usize,
    ) {
        Err(err) => {
            panic!("Couldn't allocate page {:?}", err);
        }
        Ok(_) => (),
    };

    // Ensure all memory is zeroed
    unsafe { core::ptr::write_bytes(mem_start as *mut u8, 0, size as usize) }

    // Iterate over each header
    for program_header_ptr in headers {
        // Transpose the program header as an elf header
        let program_header = unsafe {
            *(kernel_data.as_ptr().offset(program_header_ptr as isize) as *const Elf64Phdr)
        };
        if program_header.p_type == PT_LOAD {
            unsafe {
                core::ptr::copy::<u8>(
                    kernel_data
                        .as_ptr()
                        .offset(program_header.p_offset.try_into().unwrap()),
                    program_header.p_vaddr as *mut u8,
                    program_header.p_filesz.try_into().unwrap(),
                )
            }
        }
    }

    elf_header.e_entry
}
