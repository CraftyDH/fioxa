ENTRY(_start)
OUTPUT_FORMAT(elf64-x86-64)

KERNEL_START = 0xFFFFFF8000000000;
SECTIONS {
    . = KERNEL_START;
    .text ALIGN(0x1000): {
        *(.text .text.*)
    }

    .rodata ALIGN(0x1000): {
        *(.rodata.*)
    }

    .eh_frame ALIGN(0x1000): {
        *(.eh_frame)
    }

    .data ALIGN(0x1000): {
        *(.data.*)
    }

    .data.rel.ro ALIGN(0x1000): {
        *(.data.rel.ro.*)
    }

    .bss ALIGN(0x1000): {
        *(.bss.*)
    }
    KERNEL_END = .;
}