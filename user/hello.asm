bits 64

global _start

section .text

_start:
    mov rax, 1              ; SYS_WRITE
    mov rdi, 1              ; STDOUT
    lea rsi, [rel message]
    mov rdx, message_len
    syscall

    mov rax, 0              ; SYS_EXIT
    xor rdi, rdi
    syscall

.hang:
    pause
    jmp .hang

message:
    db "Hello, world!", 10
message_len equ $ - message
