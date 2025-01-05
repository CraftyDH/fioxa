use alloc::vec::Vec;
use kernel_userspace::{
    channel::{channel_create_rs, channel_read_rs, channel_write_rs, ChannelReadResult},
    elf::{validate_elf_header, Elf64Ehdr, Elf64Phdr, LoadElfError, PT_LOAD},
    message::MessageHandle,
    object::{delete_reference, KernelReference},
    process::publish_handle,
    service::serialize,
    syscall::spawn_thread,
};
use x86_64::{align_down, align_up};

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{page_mapper::PageMapping, MemoryMappingFlags},
    scheduling::{
        process::{ProcessBuilder, ProcessMemory, ProcessReferences},
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

pub fn load_elf<'a>(data: &'a [u8]) -> Result<ProcessBuilder, LoadElfError<'a>> {
    // Transpose the header as an elf header
    let elf_header = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    validate_elf_header(elf_header)?;

    let mut memory = ProcessMemory::new();

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
            memory
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
    Ok(ProcessBuilder::new(
        memory,
        elf_header.e_entry as *const u64,
        0,
    ))
}

pub fn elf_new_process_loader() {
    let (service, sright) = channel_create_rs();
    publish_handle("ELF_LOADER", sright.id());

    let mut data = Vec::with_capacity(100);
    let mut handles = Vec::with_capacity(1);
    loop {
        match channel_read_rs(service.id(), &mut data, &mut handles) {
            ChannelReadResult::Ok => (),
            _ => todo!(),
        };
        let handle = KernelReference::from_id(handles[0]);
        spawn_thread({
            move || {
                let mut data = Vec::with_capacity(100);
                let mut handles = Vec::with_capacity(2);
                match channel_read_rs(handle.id(), &mut data, &mut handles) {
                    ChannelReadResult::Ok => (),
                    ChannelReadResult::Closed => return,
                    e => {
                        warn!("{e:?}");
                        return;
                    }
                };
                if handles.len() == 0 {
                    warn!("wrong args");
                    return;
                }

                let elf = MessageHandle::from_kref(KernelReference::from_id(handles[0])).read_vec();

                let process = match load_elf(&elf) {
                    Ok(p) => p,
                    Err(err) => {
                        let msg = serialize(&err, &mut data);
                        channel_write_rs(handle.id(), msg, &[]);
                        return;
                    }
                };

                let process = process
                    .references(ProcessReferences::from_refs(&handles[1..]))
                    .build();

                for r in &handles[1..] {
                    delete_reference(*r);
                }

                let proc = with_held_interrupts(|| unsafe {
                    let thread = CPULocalStorageRW::get_current_task();
                    KernelReference::from_id(thread.process().add_value(process.into()))
                });
                channel_write_rs(handle.id(), &[], &[proc.id()]);
            }
        });
    }
}
