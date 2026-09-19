bits 64

global _start

section .text

_start:
    ; SYS_YIELD
    mov rax, 2
    syscall

    ; SYS_EXIT(42)
    mov rax, 60
    mov rdi, 42
    syscall

.hang:
    pause
    jmp .hang
