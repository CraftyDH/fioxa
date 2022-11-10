use core::arch::global_asm;

pub mod registers;

global_asm!(include_str!("ap_trampoline.asm"));

extern "C" {
    pub fn ap_trampoline();
    pub static ap_trampoline_end: u64;
}
