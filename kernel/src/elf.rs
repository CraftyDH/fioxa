use core::cmp::{max, min};

use kernel_userspace::{
    ids::ProcessID,
    service::{SendServiceMessageDest, ServiceMessage, ServiceMessageType},
    syscall::{get_pid, send_service_message, service_create, wait_receive_service_message},
};

use crate::{
    assembly::registers::Registers,
    paging::{
        page_allocator::frame_alloc_exec,
        page_table_manager::{page_4kb, Mapper},
        virt_addr_for_phys,
    },
    scheduling::{
        process::Process,
        taskmanager::{PROCESSES, TASK_QUEUE},
        without_context_switch,
    },
    service::PUBLIC_SERVICES,
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

pub fn load_elf(data: &[u8], args: &[u8]) -> ProcessID {
    // TODO: FIX
    // This is a really bad fix to an aligned start address for the buffer
    let data = data.to_vec();
    println!("LOADING...");
    // Transpose the header as an elf header
    let elf_header = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

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
        // println!("Elf Header Verified");
    } else {
        panic!("Elf Header Invalid")
    }

    let headers = (elf_header.e_phoff..((elf_header.e_phnum * elf_header.e_phentsize).into()))
        .step_by(elf_header.e_phentsize.into());

    let mut base = u64::MAX;
    let mut size = u64::MIN;

    println!("LOADING HEADERS...");
    for program_header_ptr in headers.clone() {
        let program_header =
            unsafe { *(data.as_ptr().offset(program_header_ptr as isize) as *const Elf64Phdr) };
        base = min(base, program_header.p_vaddr);
        size = max(size, program_header.p_vaddr + program_header.p_memsz);
    }

    let mem_start = (base / 0x1000) * 0x1000;
    // The size from start to finish
    let size = size - base;
    let pages = size / 4096 + 1;

    // println!(
    //     "Elf size:{size}, base:{base}, entry: {}",
    //     elf_header.e_entry
    // );

    let mut proc: Process = Process::new(crate::scheduling::process::ProcessPrivilige::USER, args);
    let map = &mut proc.page_mapper;
    println!("ALLOCING MEM...");
    let start = frame_alloc_exec(|c| c.request_cont_pages(pages as usize)).unwrap();

    println!("MAPPING MEM...");
    for page in 0..pages {
        let p = page * 0x1000;
        let page = page_4kb(start + p);

        proc.owned_pages.push(page);

        map.map_memory(page_4kb(mem_start + p), page)
            .unwrap()
            .flush();
    }

    println!("COPYING MEM...");
    // Iterate over each header
    for program_header_ptr in headers {
        // Transpose the program header as an elf header
        let program_header =
            unsafe { *(data.as_ptr().offset(program_header_ptr as isize) as *const Elf64Phdr) };
        if program_header.p_type == PT_LOAD {
            unsafe {
                core::ptr::copy_nonoverlapping::<u8>(
                    data.as_ptr()
                        .offset(program_header.p_offset.try_into().unwrap()),
                    virt_addr_for_phys(start + program_header.p_vaddr - mem_start) as *mut u8,
                    program_header.p_filesz.try_into().unwrap(),
                )
            }
        }
    }
    println!("STARTING PROC...");
    let tid = proc.new_thread_direct(elf_header.e_entry as *const u64, Registers::default());

    let pid = proc.pid;
    without_context_switch(|| {
        PROCESSES.lock().insert(proc.pid, proc);
    });
    TASK_QUEUE.push((pid, tid)).unwrap();
    pid
}

pub fn elf_new_process_loader() {
    let sid = service_create();
    let pid = get_pid();
    PUBLIC_SERVICES.lock().insert("ELF_LOADER", sid);

    loop {
        let m = wait_receive_service_message(sid);

        let query = m.get_message().unwrap();

        let resp = match query.message {
            ServiceMessageType::ElfLoader(elf, args) => {
                let pid = load_elf(elf, args);
                ServiceMessageType::ElfLoaderResp(ProcessID(pid.0))
            }
            _ => ServiceMessageType::UnknownCommand,
        };

        send_service_message(&ServiceMessage {
            service_id: sid,
            sender_pid: pid,
            tracking_number: query.tracking_number,
            destination: SendServiceMessageDest::ToProcess(query.sender_pid),
            message: resp,
        })
        .unwrap();
    }
}
