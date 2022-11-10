.global bspdone
.global aprunning
.global pml4_ptr
.global ap_startup
.global ap_trampoline
    .org 0x8000
    .code16
ap_trampoline:
    cli
    cld
    ljmp 0, 0x8040 
    .align 16
_L8010_GDT_table:
    .long 0, 0
    .long 0x0000FFFF, 0x00CF9A00
    .long 0x0000FFFF, 0x008F9200
    .long 0x00000068, 0x00CF8900
_L8030_GDT_value:
    .word _L8030_GDT_value - _L8010_GDT_table - 1
    .long 0x8010
    .long 0, 0
    .align 64
_L8040:
    // Enter protected mode
    xor    ax, ax
    mov    ax, ds
    lgdt   [0x8030]
    mov    eax, cr0
    or     eax, 1
    mov    cr0, eax
    ljmp   8, 0x8060
    .align 32
    .code32
_L8060:
    // Enter long mode
    mov ax, 16
    mov ds, ax 
    mov ss, ax 

    mov esp, 0xFC00

    mov eax, cr0
    and eax, 0x7FFFFFFF
    mov cr0, eax

    mov eax, cr4
    or eax, (1 << 5)
	mov cr4, eax

    // Set page table
    mov eax, [pml4_ptr]
	mov cr3, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax

    lgdt [GDT_PTR]
    
    push 8
    push 0x80b3
    retf

    .code64
_L80B3_start_long_mode:
    cli
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    // Get local APIC ID
    mov rax, 1
    cpuid

    shr rbx, 24
    // rdi is first parameter register
    mov rdi, rbx
    
    // Start of core local storage
    mov rax, [stack_pages + rdi * 8]
    // Set GS
    mov rdx, 0
    mov rcx, 0xC0000101
    wrmsr

    mov rsp, gs:1
    push rdi
wait_on_boot:
    pause
    cmp dword ptr bspdone, 0
    jz wait_on_boot

    lock inc dword ptr [aprunning]

    mov rax, [ap_startup]
    jmp rax
// Code shouldn't have returned
// Spin loop
    cli
spin:
    hlt
    jmp spin
// 64 bit GDT that we will use
GDT_AP:
	.quad 0
	.quad (1<<44) | (1<<47) | (1<<41) | (1<<43) | (1<<53)
	.quad (1<<44) | (1<<47) | (1<<41)
    .align 4
GDT_PTR:
    .word . - GDT_AP - 1
    .long GDT_AP
    .long 0
    .align 64
end_loc = .
.global ap_trampoline_end
ap_trampoline_end:
    .quad end_loc-ap_trampoline
stack_pages = end_loc-ap_trampoline + 0x8000
// Each cpu local information ptr will be stored here by the loader