; This is intended to be loaded at 0x8000, with necessary information placed at the bottom.
; Please recompile with NASM on modification
global ap_trampoline
[ORG 0x8000]
[BITS 16]
ap_trampoline:
    cli
    cld
    ; Enter protected mode
    xor    ax, ax
    mov    ds, ax
    lgdt   [PROTECTED_GDT.ptr]
    mov    eax, cr0
    or     eax, 1
    mov    cr0, eax

    ; Long jump to protected mode
    jmp    8:PROTECTED_MODE
[BITS 32]
PROTECTED_MODE:
    ; Enter long mode
    mov ax, 16
    mov ds, ax 
    mov ss, ax 

    mov eax, cr0
    and eax, 0x7FFFFFFF
    mov cr0, eax

    mov eax, cr4
    or eax, (1 << 5)
	mov cr4, eax

    ; Set page table
    mov eax, [pml4_ptr]
	mov cr3, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax

    lgdt [LONG_GDT.ptr]

    ; Long jump to Long mode
    jmp 8:LONG_MODE
    ; Can also use stack method    
    ; mov esp, 0xFC00
    ; push 8
    ; push _L80B3_start_long_mode
    ; retf

[BITS 64]
LONG_MODE:
    cli
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Get local APIC ID
    mov rax, 1
    cpuid

    shr rbx, 24
    ; rdi is first parameter register
    mov rdi, rbx
    
    ; Start of core local storage
    mov rax, [core_local_gs + rdi * 8]
    ; Set GS
    mov rdx, rax
    shr rdx, 32
    mov ecx, 0xC0000101
    wrmsr
    mov ecx, 0xC0000102
    wrmsr

    mov rsp, gs:0
    push rdi
wait_on_boot:
    pause
    cmp word [bspdone], 0
    jz wait_on_boot

    lock inc word [aprunning]

    mov rax, [ap_startup]
    jmp rax
; Code shouldn't have returned
; Spin loop
    cli
spin:
    hlt
    jmp spin
; 32 BIT GDT that we will temp use
PROTECTED_GDT:
    dd 0, 0
    dd 0x0000FFFF, 0x00CF9A00
    dd 0x0000FFFF, 0x008F9200
    dd 0x00000068, 0x00CF8900
.ptr:
    dw $ - PROTECTED_GDT - 1
    dd PROTECTED_GDT
; 64 bit GDT that we will temp use
LONG_GDT:
	dq 0
	dq (1<<44) | (1<<47) | (1<<41) | (1<<43) | (1<<53)
	dq (1<<44) | (1<<47) | (1<<41)
    
    ALIGN 4
    dw 0 ; Padding to make the "address of the GDT" field aligned on a 4-byte boundary
.ptr:
    dw $ - LONG_GDT - 1 
    dd LONG_GDT

    ; Align onto a 64bit boundary so that the accesses are aligned
    ALIGN 8
ap_trampoline_end equ $
; These variables will be dynamically filled in by the loader
bspdone equ ap_trampoline_end
aprunning equ ap_trampoline_end + 4
pml4_ptr equ ap_trampoline_end + 8
ap_startup equ ap_trampoline_end + 16
core_local_gs equ ap_trampoline_end + 24
; Each cpu local information ptr will be stored here by the loader
