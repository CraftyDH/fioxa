use kernel_sys::types::SyscallResult;
use rkyv::{
    Archive, Deserialize, Serialize,
    rancor::{Error, Source},
    with::InlineAsBox,
};
use thiserror::Error;

use crate::{
    handle::Handle,
    ipc::{CowAsOwned, IPCChannel},
    message::MessageHandle,
    process::ProcessHandle,
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
        return Err(LoadElfError::ElfHeaderSigInvalid);
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

#[derive(Debug, Clone, Error, Archive, Serialize, Deserialize)]
pub enum LoadElfError {
    #[error("invalid elf header signature (expected {ELF_HEADER_SIG:?})")]
    ElfHeaderSigInvalid,
    #[error("expected ET_EXEC ({ET_EXEC}), found: {0}")]
    EType(u16),
    #[error("expected EM_X86_64 ({EM_X86_64}), found: {0}")]
    EMachine(u16),
    #[error("unsupported elf version, expected 0, found: {0}")]
    ElfVersion(u32),
    #[error("internal error")]
    InternalError,
}

#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct SpawnElfProcess<'a> {
    pub elf: CowAsOwned<'a, MessageHandle>,
    #[rkyv(with = InlineAsBox)]
    pub args: &'a [u8],
    #[rkyv(with = InlineAsBox)]
    pub initial_refs: &'a [CowAsOwned<'a, Handle>],
}

pub struct ElfLoaderService(IPCChannel);

impl ElfLoaderService {
    pub fn from_channel(chan: IPCChannel) -> Self {
        Self(chan)
    }

    pub fn spawn(
        &mut self,
        elf: &MessageHandle,
        args: &[u8],
        initial_refs: &[&Handle],
    ) -> Result<ProcessHandle, LoadElfError> {
        let initial_refs: heapless::Vec<_, 31> = initial_refs.iter().map(|e| (*e).into()).collect();

        let spawn = SpawnElfProcess {
            elf: elf.into(),
            args,
            initial_refs: &initial_refs,
        };

        self.0.send(&spawn).assert_ok();
        let mut res = self.0.recv().unwrap();

        res.deserialize().unwrap()
    }
}

pub struct ElfLoaderServiceExecutor<I: ElfLoaderServiceImpl> {
    channel: IPCChannel,
    service: I,
}

impl<I: ElfLoaderServiceImpl> ElfLoaderServiceExecutor<I> {
    pub fn new(channel: IPCChannel, service: I) -> Self {
        Self { channel, service }
    }

    pub fn run(&mut self) -> Result<(), Error> {
        loop {
            let mut msg = match self.channel.recv() {
                Ok(m) => m,
                Err(SyscallResult::ChannelClosed) => return Ok(()),
                Err(e) => return Err(Error::new(e)),
            };
            let (spawn, des) = msg.access::<ArchivedSpawnElfProcess>()?;
            let elf = spawn.elf.0.deserialize(des)?;

            let hids: heapless::Vec<Handle, 31> = spawn
                .initial_refs
                .iter()
                .map(|h| h.0.deserialize(des))
                .flatten()
                .collect();

            let res = self.service.spawn(elf, &spawn.args, &hids);
            self.channel.send(&res).into_err().map_err(Error::new)?;
        }
    }
}

pub trait ElfLoaderServiceImpl {
    fn spawn(
        &mut self,
        elf: MessageHandle,
        args: &[u8],
        initial_refs: &[Handle],
    ) -> Result<ProcessHandle, LoadElfError>;
}
