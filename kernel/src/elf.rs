use alloc::sync::Arc;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_sys::types::{VMMapFlags, VMOAnonymousFlags};
use kernel_userspace::elf::{ElfLoaderServiceExecutor, ElfLoaderServiceImpl};
use kernel_userspace::service::ServiceExecutor;
use kernel_userspace::{
    elf::{Elf64Ehdr, Elf64Phdr, LoadElfError, PT_LOAD, validate_elf_header},
    handle::Handle,
    ipc::IPCChannel,
    process::ProcessHandle,
};

use x86_64::{align_down, align_up};

use crate::mutex::Spinlock;
use crate::vm::VMO;
use crate::{
    cpu_localstorage::CPULocalStorageRW,
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
    pub fn to_mapping_flags(&self) -> VMMapFlags {
        let mut flags = VMMapFlags::USERSPACE;
        if self.contains(ElfSegmentFlags::PF_W) {
            flags |= VMMapFlags::WRITEABLE;
        }
        flags
    }
}

pub fn load_elf(data: &[u8]) -> Result<ProcessBuilder, LoadElfError> {
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
            let mem = Arc::new(Spinlock::new(VMO::new_anonymous(
                size,
                VMOAnonymousFlags::empty(),
            )));

            let flags = ElfSegmentFlags::from_bits_truncate(program_header.p_flags);

            // Map into the new processes address space
            memory
                .region
                .map_vmo(mem.clone(), flags.to_mapping_flags(), Some(vstart as usize))
                .map_err(|_| LoadElfError::InternalError)?;

            unsafe {
                // Map into our address space
                let base = with_held_interrupts(|| {
                    this_mem
                        .lock()
                        .region
                        .map_vmo(mem, VMMapFlags::WRITEABLE, None)
                        .unwrap()
                });

                assert_eq!(
                    CPULocalStorageRW::hold_interrupts_depth(),
                    0,
                    "We will be causing page faults on the copy so ensure we aren't holding interrupts"
                );

                // Copy the contents
                core::ptr::copy_nonoverlapping::<u8>(
                    data.as_ptr().add(program_header.p_offset as usize),
                    (base + (program_header.p_vaddr & 0xFFF) as usize) as *mut u8,
                    program_header.p_filesz as usize,
                );

                // Unmap from our address space
                with_held_interrupts(|| this_mem.lock().region.unmap(base, size)).unwrap();
            }
        }
    }
    Ok(ProcessBuilder::new(
        memory,
        elf_header.e_entry as *const u64,
        0,
    ))
}

pub struct ElfLoader;

impl ElfLoaderServiceImpl for ElfLoader {
    fn spawn(
        &mut self,
        elf: kernel_userspace::message::MessageHandle,
        args: &[u8],
        initial_refs: &[Handle],
    ) -> Result<ProcessHandle, LoadElfError> {
        let elf = elf.read_vec();

        let process = load_elf(&elf)?;

        let hids: heapless::Vec<_, 31> = initial_refs.iter().map(|h| **h).collect();

        let process = process
            .references(ProcessReferences::from_refs(&hids))
            .args(args.to_vec())
            .build();

        let proc = with_held_interrupts(|| unsafe {
            let thread = CPULocalStorageRW::get_current_task();
            ProcessHandle::from_handle(Handle::from_id(thread.process().add_value(process.into())))
        });

        Ok(proc)
    }
}

pub fn elf_new_process_loader() {
    ServiceExecutor::with_name("ELF_LOADER", |chan| {
        sys_process_spawn_thread({
            || match ElfLoaderServiceExecutor::new(IPCChannel::from_channel(chan), ElfLoader).run()
            {
                Ok(()) => (),
                Err(e) => error!("Error running elf service: {e}"),
            }
        });
    })
    .run()
    .unwrap();
}
