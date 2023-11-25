use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_userspace::{
    elf::{validate_elf_header, Elf64Ehdr, Elf64Phdr, LoadElfError, PT_LOAD},
    ids::ProcessID,
    service::{register_public_service, SendServiceMessageDest, ServiceMessage},
    syscall::{get_pid, receive_service_message_blocking, send_service_message, service_create},
};
use x86_64::{align_down, align_up};

use crate::{
    assembly::registers::Registers,
    paging::{
        page_allocator::frame_alloc_exec,
        page_mapper::{MaybeAllocatedPage, PageMapping},
        virt_addr_for_phys,
    },
    scheduling::{
        process::{Process, ProcessPrivilige},
        taskmanager::{push_task_queue, PROCESSES},
        without_context_switch,
    },
};

pub fn load_elf<'a>(
    data: &'a [u8],
    args: &[u8],
    kernel: bool,
) -> Result<ProcessID, LoadElfError<'a>> {
    // Transpose the header as an elf header
    let elf_header = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    validate_elf_header(&elf_header)?;

    let process = Process::new(
        if kernel {
            ProcessPrivilige::KERNEL
        } else {
            ProcessPrivilige::USER
        },
        args,
    );

    let headers = (elf_header.e_phoff..((elf_header.e_phnum * elf_header.e_phentsize).into()))
        .step_by(elf_header.e_phentsize.into())
        // Transpose the program header as an elf header
        .map(|header| unsafe { &*(data.as_ptr().add(header as usize) as *const Elf64Phdr) });

    let mut memory = process.memory.lock();

    println!("COPYING MEM...");
    // Iterate over each header
    for program_header in headers {
        if program_header.p_type == PT_LOAD {
            let vstart = align_down(program_header.p_vaddr, 0x1000);
            let vallocend = align_up(program_header.p_vaddr + program_header.p_filesz, 0x1000);
            let vend = align_up(program_header.p_vaddr + program_header.p_memsz, 0x1000);

            let pages: Box<[_]> =
                frame_alloc_exec(|c| c.request_cont_pages((vallocend - vstart) as usize / 0x1000))
                    .ok_or(LoadElfError::InternalError)?
                    .map(|a| MaybeAllocatedPage::from(a))
                    // if there is extra space lazy allocate the zeros
                    .chain((0..((vend - vallocend) / 0x1000)).map(|_| MaybeAllocatedPage::new()))
                    .collect();

            let first = pages[0].get();
            memory
                .page_mapper
                .insert_mapping_at(vstart as usize, PageMapping::new_lazy_prealloc(pages))
                .unwrap();

            // If all zeros we don't need to copy anything
            if let Some(first) = first {
                let start = first.get_address();
                unsafe {
                    core::ptr::copy_nonoverlapping::<u8>(
                        data.as_ptr().add(program_header.p_offset as usize),
                        virt_addr_for_phys(start + program_header.p_vaddr - vstart) as *mut u8,
                        program_header.p_filesz as usize,
                    )
                }
            }
        }
    }
    drop(memory);
    println!("STARTING PROC...");
    let tid = process.new_thread_direct(elf_header.e_entry as *const u64, Registers::default());
    let thread = Arc::downgrade(&tid);
    let pid = process.pid;
    without_context_switch(|| {
        PROCESSES.lock().insert(pid, process);
    });
    push_task_queue(thread).expect("thread should be able enter the queue");
    Ok(pid)
}

pub fn elf_new_process_loader() {
    let sid = service_create();
    let pid = get_pid();
    register_public_service("ELF_LOADER", sid, &mut Vec::new());

    let mut message_buffer = Vec::new();
    let mut send_buffer = Vec::new();
    loop {
        let query: ServiceMessage<(&[u8], &[u8], bool)> =
            receive_service_message_blocking(sid, &mut message_buffer).unwrap();

        let (elf, args, kernel) = query.message;

        println!("LOADING...");

        let resp = load_elf(elf, args, kernel);

        send_service_message(
            &ServiceMessage {
                service_id: sid,
                sender_pid: pid,
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
                message: resp,
            },
            &mut send_buffer,
        )
        .unwrap();
    }
}
