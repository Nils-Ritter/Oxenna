bits 64

global _start

section .text

_start:
.loop:
    pause
    jmp .loop
