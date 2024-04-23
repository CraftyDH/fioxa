use alloc::vec::Vec;
use kernel_userspace::{
    elf::{validate_elf_header, Elf64Ehdr, Elf64Phdr, LoadElfError, PT_LOAD},
    ids::ProcessID,
    service::{make_message, register_public_service, SendServiceMessageDest, ServiceMessageDesc},
    syscall::{get_pid, receive_service_message_blocking, send_service_message, service_create},
};
use x86_64::{align_down, align_up, instructions::interrupts::without_interrupts};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    paging::{page_mapper::PageMapping, MemoryMappingFlags},
    scheduling::{
        process::{Process, ProcessPrivilige},
        taskmanager::{push_task_queue, PROCESSES},
        without_context_switch,
    },
};

bitflags::bitflags! {
    struct ElfSegmentFlags: u32 {
        const PF_X = 0x1;
        const PF_W = 0x2;
        const PF_R = 0x4;
    }
}

impl ElfSegmentFlags {
    pub fn to_mapping_flags(&self) -> MemoryMappingFlags {
        let mut flags = MemoryMappingFlags::USERSPACE;
        if self.contains(ElfSegmentFlags::PF_W) {
            flags |= MemoryMappingFlags::WRITEABLE;
        }
        flags
    }
}

pub fn load_elf<'a>(
    data: &'a [u8],
    args: &[u8],
    kernel: bool,
) -> Result<ProcessID, LoadElfError<'a>> {
    // Transpose the header as an elf header
    let elf_header = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    validate_elf_header(elf_header)?;

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

    let this_mem = unsafe { &CPULocalStorageRW::get_current_task().process().memory };

    println!("COPYING MEM...");
    // Iterate over each header
    for program_header in headers {
        if program_header.p_type == PT_LOAD {
            let vstart = align_down(program_header.p_vaddr, 0x1000);
            // let vallocend = align_up(program_header.p_vaddr + program_header.p_filesz, 0x1000);
            let vend = align_up(program_header.p_vaddr + program_header.p_memsz, 0x1000);

            let size = (vend - vstart) as usize;
            let mem = PageMapping::new_lazy(size);

            let flags = ElfSegmentFlags::from_bits_truncate(program_header.p_flags);

            // Map into the new processes address space
            memory
                .page_mapper
                .insert_mapping_at(vstart as usize, mem.clone(), flags.to_mapping_flags())
                .ok_or(LoadElfError::InternalError)?;

            unsafe {
                // Map into our address space
                let base = without_interrupts(|| {
                    this_mem
                        .lock()
                        .page_mapper
                        .insert_mapping(mem, MemoryMappingFlags::all())
                });

                // Copy the contents
                core::ptr::copy_nonoverlapping::<u8>(
                    data.as_ptr().add(program_header.p_offset as usize),
                    (base + (program_header.p_vaddr & 0xFFF) as usize) as *mut u8,
                    program_header.p_filesz as usize,
                );

                // Unmap from our address space
                without_interrupts(|| this_mem.lock().page_mapper.free_mapping(base..base + size))
                    .unwrap();
            }
        }
    }
    drop(memory);
    println!("STARTING PROC...");
    let thread = process.new_thread_direct(elf_header.e_entry as *const u64, Registers::default());
    let pid = process.pid;
    without_context_switch(|| {
        PROCESSES.lock().insert(pid, process);
    });
    push_task_queue(thread);
    Ok(pid)
}

pub fn elf_new_process_loader() {
    let sid = service_create();
    let pid = get_pid();
    register_public_service("ELF_LOADER", sid, &mut Vec::new());

    let mut message_buffer = Vec::new();
    let mut send_buffer = Vec::new();
    loop {
        let query = receive_service_message_blocking(sid);

        let (elf, args, kernel): (&[u8], &[u8], bool) = query.read(&mut message_buffer).unwrap();

        println!("LOADING...");

        let resp = load_elf(elf, args, kernel);

        send_service_message(
            &ServiceMessageDesc {
                service_id: sid,
                sender_pid: pid,
                tracking_number: query.tracking_number,
                destination: SendServiceMessageDest::ToProcess(query.sender_pid),
            },
            &make_message(&resp, &mut send_buffer),
        );
    }
}
