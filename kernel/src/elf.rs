use core::cmp::{max, min};

use alloc::{sync::Arc, vec::Vec};
use kernel_userspace::{
    elf::{validate_elf_header, Elf64Ehdr, Elf64Phdr, LoadElfError, PT_LOAD},
    ids::ProcessID,
    service::{register_public_service, SendServiceMessageDest, ServiceMessage},
    syscall::{get_pid, receive_service_message_blocking, send_service_message, service_create},
};

use crate::{
    assembly::registers::Registers,
    paging::{
        page_allocator::frame_alloc_exec,
        page_table_manager::{Mapper, Page},
        virt_addr_for_phys,
    },
    scheduling::{
        process::Process,
        taskmanager::{push_task_queue, PROCESSES},
        without_context_switch,
    },
};

pub fn load_elf<'a>(data: &'a [u8], args: &[u8]) -> Result<ProcessID, LoadElfError<'a>> {
    // Transpose the header as an elf header
    let elf_header = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    validate_elf_header(elf_header)?;

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
    let pages_count = size / 4096 + 1;

    let process = Process::new(crate::scheduling::process::ProcessPrivilige::USER, args);
    println!("ALLOCING MEM...");
    let mut pages = frame_alloc_exec(|c| c.request_cont_pages(pages_count as usize))
        .unwrap()
        .peekable();

    assert_eq!(pages.len(), pages_count as usize);
    let start = pages.peek().unwrap().get_address();

    {
        let mut memory = process.memory.lock();

        for (page, virt_addr) in pages.zip((mem_start..).step_by(0x1000)) {
            memory
                .page_mapper
                .map_memory(Page::new(virt_addr), *page)
                .unwrap()
                .flush();
            memory.owned_pages.push(page);
        }
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
    let tid = process.new_thread_direct(elf_header.e_entry as *const u64, Registers::default());
    let thread = Arc::downgrade(&tid);
    let pid = process.pid;
    without_context_switch(|| {
        PROCESSES.lock().insert(pid, process);
    });
    push_task_queue(thread).unwrap();
    Ok(pid)
}

pub fn elf_new_process_loader() {
    let sid = service_create();
    let pid = get_pid();
    register_public_service("ELF_LOADER", sid, &mut Vec::new());

    let mut message_buffer = Vec::new();
    let mut tmp_prog_buffer = Vec::new();
    loop {
        let query: ServiceMessage<(&[u8], &[u8])> =
            receive_service_message_blocking(sid, &mut message_buffer).unwrap();

        let (elf, args) = query.message;

        // TODO: FIX
        // This is a really bad fix to an aligned start address for the buffer
        // let data = data.to_vec();
        tmp_prog_buffer.reserve(elf.len());
        unsafe {
            tmp_prog_buffer.set_len(elf.len());
        }
        tmp_prog_buffer.copy_from_slice(elf);
        println!("LOADING...");

        let resp = load_elf(&tmp_prog_buffer, args);

        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: pid,
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
                message: resp,
            },
            &mut message_buffer,
        )
        .unwrap();
    }
}
