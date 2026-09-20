.section .text
.global _start
.type _start, @function

_start:
    # write(1, message, message_len)
    mov $1, %rax
    mov $1, %rdi
    lea message(%rip), %rsi
    mov $(message_end - message), %rdx
    syscall

    # exit(0)
    mov $60, %rax
    xor %rdi, %rdi
    syscall

    hlt

.section .rodata
message:
    .ascii "Hello from an Oxenna .ox application!\n"
message_end:
