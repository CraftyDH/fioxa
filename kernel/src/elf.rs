use alloc::sync::Arc;
use alloc::vec::Vec;
use bootloader::BootInfo;
use bootloader::uefi::boot::{MemoryDescriptor, MemoryType};
use elf::abi::{EM_X86_64, ET_EXEC, PT_LOAD};
use elf::endian::NativeEndian;
use kernel_sys::syscall::sys_process_spawn_thread;
use kernel_sys::types::{VMMapFlags, VMOAnonymousFlags};
use kernel_userspace::elf::{ElfLoaderServiceExecutor, ElfLoaderServiceImpl};
use kernel_userspace::service::ServiceExecutor;
use kernel_userspace::{
    elf::LoadElfError, handle::Handle, ipc::IPCChannel, process::ProcessHandle,
};

use spin::Lazy;
use x86_64::{align_down, align_up};

use crate::BOOT_INFO;
use crate::interrupts::execute_kexec;
use crate::ioapic::disable_apic;
use crate::lapic::disable_localapic;
use crate::memory::MemoryMapIter;
use crate::mutex::Spinlock;
use crate::paging::offset_map::map_gop;
use crate::paging::page::{Page, Size4KB};
use crate::paging::page_allocator::global_allocator;
use crate::paging::page_table::{
    MapMemoryError, Mapper, PageTable, TableLevel, TableLevel3, TableLevel4,
};
use crate::paging::{
    MemoryLoc, OFFSET_MAP, PageAllocator, get_mem_offset, virt_addr_for_phys, virt_addr_offset,
};
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
        const PF_X = ::elf::abi::PF_X;
        const PF_W = ::elf::abi::PF_W;
        const PF_R = ::elf::abi::PF_R;
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
    let elf_file = ::elf::ElfBytes::<NativeEndian>::minimal_parse(data).map_err(|e| {
        info!("error: {e}");
        LoadElfError::InternalError
    })?;

    if elf_file.ehdr.e_type != ET_EXEC || elf_file.ehdr.e_machine != EM_X86_64 {
        return Err(LoadElfError::InternalError);
    }

    let mut memory = ProcessMemory::new();
    let this_mem = unsafe { &CPULocalStorageRW::get_current_task().process().memory };

    let segments = elf_file.segments().ok_or(LoadElfError::InternalError)?;

    // Iterate over each header
    for program_header in segments {
        if program_header.p_type == PT_LOAD {
            // If a likely kernel use kexec
            // TODO: Add proper path to invoke.
            if program_header.p_paddr >= MemoryLoc::EndUserMem as u64 {
                load_kernel(data);
            }
            let data = elf_file.segment_data(&program_header).map_err(|e| {
                info!("error: {e}");
                LoadElfError::InternalError
            })?;

            let vstart = align_down(program_header.p_vaddr, 0x1000);
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
                    data.as_ptr(),
                    (base + (program_header.p_vaddr & 0xFFF) as usize) as *mut u8,
                    data.len(),
                );

                // Unmap from our address space
                with_held_interrupts(|| this_mem.lock().region.unmap(base, size)).unwrap();
            }
        }
    }
    Ok(ProcessBuilder::new(
        memory,
        elf_file.ehdr.e_entry as *const u64,
        0,
    ))
}

pub fn load_kernel(data: &[u8]) {
    let elf_file = ::elf::ElfBytes::<NativeEndian>::minimal_parse(data).unwrap();

    // Construct a memory map from the initial one with everything we allocate for the new kernel here subtracted
    let mut mmap = unsafe {
        let boot_info = &*virt_addr_offset(BOOT_INFO);
        MemoryMapIter::new(
            virt_addr_offset(boot_info.mmap_buf),
            boot_info.mmap_entry_size,
            boot_info.mmap_len,
        )
        .map(|e| *e)
        .collect::<Vec<_>>()
    };

    let mem_key =
        MemoryType::custom(mmap.iter().map(|e| e.ty.0).max().unwrap().max(0x80000000) + 1);

    mmap.sort_by_key(|e| e.phys_start);

    let remove_page = |mmap: &mut Vec<MemoryDescriptor>, page: u64| {
        for i in 0..mmap.len() {
            let e = mmap[i];
            let start = e.phys_start;
            let end = start + e.page_count * 0x1000;

            if page < start || page >= end || e.ty == mem_key {
                continue;
            }

            assert_eq!(e.ty, MemoryType::CONVENTIONAL);

            if mmap[i].page_count == 1 {
                mmap[i].ty = mem_key;
            } else if page == start {
                mmap[i].page_count -= 1;
                mmap[i].phys_start += 0x1000;

                // try to use prev
                if let Some(prev) = mmap.get_mut(i - 1)
                    && prev.ty == mem_key
                    && prev.phys_start + prev.page_count * 0x1000 == page
                {
                    prev.page_count += 1;
                } else {
                    mmap.insert(
                        i,
                        MemoryDescriptor {
                            ty: mem_key,
                            phys_start: page,
                            virt_start: 0,
                            page_count: 1,
                            att: e.att,
                        },
                    );
                };
            } else if page + 0x1000 == end {
                mmap[i].page_count -= 1;

                // try to use next
                if let Some(next) = mmap.get_mut(i + 1)
                    && next.ty == mem_key
                    && next.phys_start == page + 0x1000
                {
                    next.phys_start = page;
                    next.page_count += 1;
                } else {
                    mmap.insert(
                        i + 1,
                        MemoryDescriptor {
                            ty: mem_key,
                            phys_start: page,
                            virt_start: 0,
                            page_count: 1,
                            att: e.att,
                        },
                    );
                };
            } else {
                let new = (page - start) / 0x1000;
                mmap[i].page_count = new;
                mmap.insert(
                    i + 1,
                    MemoryDescriptor {
                        ty: mem_key,
                        phys_start: page,
                        virt_start: 0,
                        page_count: 1,
                        att: e.att,
                    },
                );
                mmap.insert(
                    i + 2,
                    MemoryDescriptor {
                        ty: e.ty,
                        phys_start: page + 0x1000,
                        virt_start: 0,
                        page_count: e.page_count - new - 1,
                        att: e.att,
                    },
                );
            }
        }
    };

    let remove_pages = |mmap: &mut Vec<MemoryDescriptor>, page: u64, count: u64| {
        for i in 0..count {
            remove_page(mmap, page + i * 0x1000);
        }
    };

    let mut bootsp = PageTable::<TableLevel4>::new(global_allocator());
    remove_page(&mut mmap, bootsp.get_physical_address() as u64);

    unsafe {
        let boot_info = &*virt_addr_offset(BOOT_INFO);
        map_gop(global_allocator(), &mut bootsp, &boot_info.gop);
    }

    let set = |addr, t: &Lazy<Spinlock<PageTable<TableLevel3>>>| {
        let e = &mut bootsp.table().entries[TableLevel4::calculate_index(addr)];
        e.set_present(true);
        e.set_read_write(true);
        e.set_user_super(false);
        e.set_address(t.lock().get_physical_address() as u64);
    };
    set(MemoryLoc::PhysMapOffset as usize, &OFFSET_MAP);

    let this_mem = unsafe { &CPULocalStorageRW::get_current_task().process().memory };

    // Iterate over each header
    for program_header in elf_file.segments().unwrap() {
        if program_header.p_type == PT_LOAD {
            let data = elf_file.segment_data(&program_header).unwrap();

            let vstart = align_down(program_header.p_vaddr, 0x1000);
            let vend = align_up(program_header.p_vaddr + program_header.p_memsz, 0x1000);

            let size = (vend - vstart) as usize;
            let mem = Arc::new(Spinlock::new(VMO::new_anonymous(
                size,
                VMOAnonymousFlags::PINNED,
            )));

            core::mem::forget(mem.clone());

            let flags = ElfSegmentFlags::from_bits_truncate(program_header.p_flags);

            let mut vmflags = VMMapFlags::empty();

            if flags.contains(ElfSegmentFlags::PF_W) {
                vmflags |= VMMapFlags::WRITEABLE;
            }

            // Map into the new processes address space
            for (i, page) in mem.lock().vmo_pages_mut().iter_mut().enumerate() {
                let p = page.unwrap();

                match bootsp.map(
                    global_allocator(),
                    Page::new(vstart + i as u64 * 0x1000),
                    p,
                    vmflags,
                ) {
                    Ok(_) => remove_page(&mut mmap, p.get_address()),
                    Err(MapMemoryError::MemAlreadyMapped { current, .. }) => {
                        // TODO: Fix
                        error!("mem already mapped?");
                        *page = Some(Page::new(current))
                    }
                }
            }

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
                    data.as_ptr(),
                    (base + (program_header.p_vaddr & 0xFFF) as usize) as *mut u8,
                    data.len(),
                );

                // Unmap from our address space
                with_held_interrupts(|| this_mem.lock().region.unmap(base, size)).unwrap();
            }
        }
    }

    let stack = global_allocator().allocate_pages(16).unwrap();
    remove_pages(&mut mmap, stack.get_address(), 16);

    Mapper::<Size4KB>::get_page_addresses(&bootsp, |p| remove_page(&mut mmap, p.get_address()));

    // Add an extra page to buffer incase allocating space for the map increases mapping count
    let pages = ((mmap.len() * size_of::<MemoryDescriptor>()).div_ceil(0x1000)) + 1;
    let boot_stuff = global_allocator().allocate_page().unwrap();
    remove_page(&mut mmap, boot_stuff.get_address());

    let mmap_addr = global_allocator().allocate_pages(pages).unwrap();
    remove_pages(&mut mmap, mmap_addr.get_address(), pages as u64);

    let pages2 = (mmap.len() * size_of::<MemoryDescriptor>()).div_ceil(0x1000);
    assert!(pages2 <= pages, "somehow we didn't allocate enough");
    unsafe {
        let mmap_ptrs = virt_addr_for_phys(mmap_addr.get_address()) as *mut MemoryDescriptor;
        let mmap_slice = core::slice::from_raw_parts_mut(mmap_ptrs, mmap.len());
        mmap_slice.copy_from_slice(&mmap);
    }

    unsafe {
        let info = &*virt_addr_offset(BOOT_INFO);
        let new_info = &mut *(virt_addr_for_phys(boot_stuff.get_address()) as *mut BootInfo);
        new_info.gop = info.gop.clone();
        new_info.mmap_buf = mmap_addr.get_address() as *mut u8;
        new_info.mmap_len = mmap.len();
        new_info.mmap_entry_size = size_of::<MemoryDescriptor>();
        new_info.uefi_runtime_table = info.uefi_runtime_table;
        new_info.offset = get_mem_offset();
    }

    unsafe { disable_apic() };

    let arg_cr3 = bootsp.get_physical_address();
    let arg_stack = virt_addr_for_phys(stack.get_address() + 16 * 0x1000);
    let arg_entry = elf_file.ehdr.e_entry;
    let arg_bootinfo = virt_addr_for_phys(boot_stuff.get_address());

    execute_kexec(move || unsafe {
        disable_localapic();
        core::arch::asm!(
            "mov cr3, {}; mov rsp, {}; push 0; jmp {}",
            in(reg) arg_cr3,
            in(reg) arg_stack,
            in(reg) arg_entry,
            in("rdi") arg_bootinfo,
            options(noreturn)
        )
    })
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
