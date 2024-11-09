use alloc::{sync::Arc, vec::Vec};
use kernel_userspace::{
    elf::{validate_elf_header, Elf64Ehdr, Elf64Phdr, LoadElfError, SpawnElfProcess, PT_LOAD},
    message::MessageHandle,
    object::{KernelObjectType, KernelReference},
    service::{deserialize, make_message_new},
    socket::{SocketHandle, SocketListenHandle},
    syscall::spawn_thread,
};
use x86_64::{align_down, align_up};

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{page_mapper::PageMapping, MemoryMappingFlags},
    scheduling::{
        process::{Process, ProcessPrivilige},
        taskmanager::{push_task_queue, PROCESSES},
        with_held_interrupts,
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
    references: &[KernelReference],
    kernel: bool,
) -> Result<Arc<Process>, LoadElfError<'a>> {
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

    // build initial refs
    with_held_interrupts(|| unsafe {
        let mut refs = process.references.lock();
        let this = CPULocalStorageRW::get_current_task();
        let mut this_refs = this.process().references.lock();
        for r in references {
            refs.add_value(
                this_refs
                    .references()
                    .get(&r.id())
                    .expect("loader proc should have ref in its map")
                    .clone(),
            );
        }
    });

    let headers = (elf_header.e_phoff..((elf_header.e_phnum * elf_header.e_phentsize).into()))
        .step_by(elf_header.e_phentsize.into())
        // Transpose the program header as an elf header
        .map(|header| unsafe { &*(data.as_ptr().add(header as usize) as *const Elf64Phdr) });

    let this_mem = unsafe { &CPULocalStorageRW::get_current_task().process().memory };

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
            process
                .memory
                .lock()
                .page_mapper
                .insert_mapping_at(vstart as usize, mem.clone(), flags.to_mapping_flags())
                .ok_or(LoadElfError::InternalError)?;

            unsafe {
                // Map into our address space
                let base = with_held_interrupts(|| {
                    this_mem
                        .lock()
                        .page_mapper
                        .insert_mapping(mem, MemoryMappingFlags::all())
                });

                assert_eq!(CPULocalStorageRW::hold_interrupts_depth(), 0, "We will be causing page faults on the copy so ensure we aren't holding interrupts");

                // Copy the contents
                core::ptr::copy_nonoverlapping::<u8>(
                    data.as_ptr().add(program_header.p_offset as usize),
                    (base + (program_header.p_vaddr & 0xFFF) as usize) as *mut u8,
                    program_header.p_filesz as usize,
                );

                // Unmap from our address space
                with_held_interrupts(|| {
                    this_mem.lock().page_mapper.free_mapping(base..base + size)
                })
                .unwrap();
            }
        }
    }
    let thread = process.new_thread(elf_header.e_entry as *const u64, 0);
    with_held_interrupts(|| {
        PROCESSES.lock().insert(process.pid, process.clone());
    });
    push_task_queue(thread.expect("new process shouldn't have died"));
    Ok(process)
}

pub fn elf_new_process_loader() {
    let service = SocketListenHandle::listen("ELF_LOADER").unwrap();
    loop {
        let job = service.blocking_accept();
        spawn_thread(|| match elf_loader_handler(job) {
            Ok(()) => (),
            Err(e) => error!("Error handling elf load: {e}"),
        });
    }
}

fn elf_loader_handler(handle: SocketHandle) -> Result<(), &'static str> {
    let (descriptor, desc_type) = handle
        .blocking_recv()
        .map_err(|_| "Couldn't get descriptor")?;

    if desc_type != KernelObjectType::Message {
        return Err("Desc type not message");
    }

    let descriptor = MessageHandle::from_kref(descriptor);

    let descriptor_vec = descriptor.read_vec();

    let spawn: SpawnElfProcess = deserialize(&descriptor_vec).map_err(|_| "error deserializing")?;

    let (elf, elf_type) = handle.blocking_recv().map_err(|_| "couldn't get elf")?;

    if elf_type != KernelObjectType::Message {
        return Err("Elf type not message");
    }

    let elf = MessageHandle::from_kref(elf);
    let elf_data = elf.read_vec();

    let mut refs = Vec::with_capacity(spawn.init_references_count);

    for _ in 0..spawn.init_references_count {
        let (desc, _) = handle
            .blocking_recv()
            .map_err(|_| "couldn't get user descriptor")?;
        refs.push(desc);
    }

    let res = load_elf(&elf_data, spawn.args, &refs, false);

    match res {
        Ok(proc) => {
            let proc = with_held_interrupts(|| unsafe {
                let thread = CPULocalStorageRW::get_current_task();
                KernelReference::from_id(thread.process().add_value(proc.into()))
            });
            handle.blocking_send(&proc).map_err(|_| "sock closed")?;
        }
        Err(err) => {
            let msg = make_message_new(&err);
            handle
                .blocking_send(msg.kref())
                .map_err(|_| "sock closed")?;
        }
    }

    Ok(())
}
