use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    message::MessageHandle,
    object::{KernelObjectType, KernelReferenceID},
    process::ProcessHandle,
    service::{deserialize, make_message},
    socket::SocketHandle,
};

#[repr(C, packed)]
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

#[repr(C, packed)]
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
pub const ELFCLASS64: u8 = 2; // 64 BIT
pub const ELFDATA2LSB: u8 = 1; // LSB not MSB

pub const ET_EXEC: u16 = 2; // Executable file
pub const EM_X86_64: u16 = 62; // AMD x86-64 architecture

// For the ELF Program Header https://refspecs.linuxbase.org/elf/gabi4+/ch5.pheader.html
pub const PT_LOAD: u32 = 1; // A loadable segment

pub const ELF_HEADER_SIG: [u8; 6] = [0x7F, b'E', b'L', b'F', ELFCLASS64, ELFDATA2LSB];

pub fn validate_elf_header(elf_header: &Elf64Ehdr) -> Result<(), LoadElfError> {
    // Ensure that all the header flags are suitable
    if elf_header.e_ident[0..6] != ELF_HEADER_SIG {
        return Err(LoadElfError::ElfHeaderSigInvalid(&elf_header.e_ident[0..6]));
    }
    if elf_header.e_type != ET_EXEC {
        return Err(LoadElfError::EType(elf_header.e_type));
    }
    if elf_header.e_machine != EM_X86_64 {
        return Err(LoadElfError::EMachine(elf_header.e_machine));
    }
    if elf_header.e_version != 1 {
        return Err(LoadElfError::ElfVersion(elf_header.e_version));
    }
    Ok(())
}

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum LoadElfError<'a> {
    #[error("invalid elf header signature (expected {ELF_HEADER_SIG:?}, found {0:?})")]
    ElfHeaderSigInvalid(&'a [u8]),
    #[error("expected ET_EXEC ({ET_EXEC}), found: {0}")]
    EType(u16),
    #[error("expected EM_X86_64 ({EM_X86_64}), found: {0}")]
    EMachine(u16),
    #[error("unsupported elf version, expected 0, found: {0}")]
    ElfVersion(u32),
    #[error("internal error")]
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnElfProcess<'a> {
    pub args: &'a [u8],
    pub init_references_count: usize,
}

pub fn spawn_elf_process<'a>(
    elf: MessageHandle,
    args: &[u8],
    initial_refs: &[KernelReferenceID],
    buffer: &'a mut Vec<u8>,
) -> Result<ProcessHandle, LoadElfError<'a>> {
    let msg = make_message(
        &SpawnElfProcess {
            args,
            init_references_count: initial_refs.len(),
        },
        buffer,
    );

    let socket = SocketHandle::connect("ELF_LOADER").unwrap();
    socket.blocking_send(msg.kref()).unwrap();
    socket.blocking_send(elf.kref()).unwrap();
    for r in initial_refs {
        socket.blocking_send_raw(*r).unwrap();
    }
    let (resp, resp_ty) = socket.blocking_recv().unwrap();

    match resp_ty {
        KernelObjectType::Process => Ok(ProcessHandle::from_kref(resp)),
        KernelObjectType::Message => {
            let resp = MessageHandle::from_kref(resp);
            resp.read_into_vec(buffer);
            Err(deserialize(buffer).unwrap())
        }
        _ => panic!("bad response"),
    }
}
