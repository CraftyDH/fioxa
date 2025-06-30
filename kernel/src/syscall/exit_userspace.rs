use x86_64::structures::idt::InterruptStackFrame;

use crate::{cpu_localstorage::CPULocalStorage, scheduling::process::VMExitStateID};

/// Handler for internal syscalls called by the kernel (Note: not nested).
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_kernel_handler() {
    /// Function used to ensure kernel doesn't call syscall while holding interrupts
    extern "C" fn bad_interrupts_held() {
        panic!("Interrupts should not be held when entering syscall.")
    }
    core::arch::naked_asm!(
        // check interrupts
        "cmp qword ptr gs:{int_depth}, 0",
        "jne {bad_interrupts_held}",

        // save regs
        "pushfq",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Start saved kernel struct
        "push rsp",
        // args
        "push r9",
        "push r8",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        // number
        "push rax",

        "mov rdx, rsp",

        "mov rdx, rsp",
        "mov rsp, gs:{vm_sp}",
        "mov eax, {exit_type}",
        "ret",
        bad_interrupts_held = sym bad_interrupts_held,
        int_depth = const core::mem::offset_of!(CPULocalStorage, hold_interrupts_depth),
        vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
        exit_type = const VMExitStateID::Kernel as u32,
    );
}

/// Handler for syscalls via int 0x80
#[unsafe(naked)]
pub extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    core::arch::naked_asm!(
        // preserved
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // args
        "push r9",
        "push r8",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        // number
        "push rax",

        "mov rdx, rsp",
        "mov rsp, gs:{vm_sp}",
        "mov eax, {exit_type}",
        "ret",
        vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
        exit_type = const VMExitStateID::IntSyscall as u32,
    );
}

/// Handler for syscalls via syscall
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_sysret_handler() {
    core::arch::naked_asm!(
        // swap stack
        "mov r12, rsp",
        "mov rsp, gs:{scratch}",

        "push r12", // RSP
        "push r11", // RFLAGS
        "push rcx", // RIP

        // preserved
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // args
        "push r9",
        "push r8",
        "push r10", // rcx (move arg3 to match sysv c calling convention)
        "push rdx",
        "push rsi",
        "push rdi",
        // number
        "push rax",

        "mov rdx, rsp",
        "mov rsp, gs:{vm_sp}",
        "mov eax, {exit_type}",
        "ret",
        scratch = const core::mem::offset_of!(CPULocalStorage, scratch_stack_top),
        vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
        exit_type = const VMExitStateID::Syscall as u32,
    );
}
