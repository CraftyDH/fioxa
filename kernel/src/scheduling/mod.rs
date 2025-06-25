use crate::cpu_localstorage::CPULocalStorageRW;

pub mod process;
pub mod taskmanager;

pub fn with_held_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe {
        CPULocalStorageRW::inc_hold_interrupts();

        let tmp = f();

        CPULocalStorageRW::dec_hold_interrupts();

        tmp
    }
}
