.intel_syntax noprefix

.global _start

.section .text

_start:
    # ------------------------------------------------------------
    # OS
    # ------------------------------------------------------------

    lea rsi, [rip + os]
    mov edx, 21
    mov edi, 1
    mov eax, 1
    syscall

    # ------------------------------------------------------------
    # Kernel
    # ------------------------------------------------------------

    lea rsi, [rip + kernel]
    mov edx, 28
    mov edi, 1
    mov eax, 1
    syscall

    # ------------------------------------------------------------
    # Bootloader
    # ------------------------------------------------------------

    lea rsi, [rip + bootloader]
    mov edx, 21
    mov edi, 1
    mov eax, 1
    syscall

    # ------------------------------------------------------------
    # Architecture
    # ------------------------------------------------------------

    lea rsi, [rip + architecture]
    mov edx, 21
    mov edi, 1
    mov eax, 1
    syscall

    # ------------------------------------------------------------
    # Exit
    # ------------------------------------------------------------

    xor edi, edi
    mov eax, 60
    syscall

    hlt


.section .rodata

os:
    .ascii "OS:           Oxenna\n"

kernel:
    .ascii "Kernel:       Oxenna x86_64\n"

bootloader:
    .ascii "Bootloader:   Limine\n"

architecture:
    .ascii "Architecture: x86_64\n"
