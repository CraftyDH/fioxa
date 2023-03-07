pub const SYSCALL_NUMBER: usize = 0x80;

// Syscall
// RAX: number
//

// Syscalls
pub const ECHO: usize = 0;
pub const YIELD_NOW: usize = 1;
pub const SPAWN_PROCESS: usize = 2;
pub const SPAWN_THREAD: usize = 3;
pub const SLEEP: usize = 4;
pub const EXIT_THREAD: usize = 5;
pub const MMAP_PAGE: usize = 6;
pub const STREAM: usize = 7;

pub const STREAM_CONNECT: usize = 0;
pub const STREAM_PUSH: usize = 1;
pub const STREAM_POP: usize = 2;

pub const READ_ARGS: usize = 8;
