use core::arch::global_asm;

/// System call numbers.
///
/// These numbers are part of the userspace ABI. Once userspace
/// programs depend on them, don't casually renumber them.
pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 2;

/// Standard output.
pub const STDOUT: u64 = 1;

/// Standard error.
pub const STDERR: u64 = 2;

/// Return value for an unsupported syscall.
///
/// This is equivalent to -38 interpreted as u64.
pub const ENOSYS: u64 = (-38i64) as u64;

/*
 * Registers saved by the syscall entry stub.
 *
 * The order MUST exactly match the push order in
 * `oxenna_syscall_entry`.
 */
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,

    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/*
 * SYSCALL does not save RSP.
 *
 * Save the userspace RSP before switching to the kernel stack.
 *
 * This is currently global because Oxenna is single-core.
 * This should become per-CPU state once SMP is implemented.
 */
#[unsafe(no_mangle)]
pub static mut SYSCALL_USER_RSP: u64 = 0;

/// Assembly entry point for SYSCALL.
///
/// This is implemented by `global_asm!` below. The declaration
/// here makes the linker symbol visible to Rust so that gdt.rs
/// can put its address into IA32_LSTAR.
unsafe extern "C" {
    pub fn oxenna_syscall_entry();
}

pub fn syscall_entry_address() -> x86_64::VirtAddr {
    x86_64::VirtAddr::from_ptr(
        oxenna_syscall_entry as *const ()
    )
}

/*
 * x86-64 SYSCALL entry point.
 *
 * On entry from userspace:
 *
 *     RCX = userspace RIP
 *     R11 = userspace RFLAGS
 *     RSP = userspace RSP
 *
 * SYSCALL also changes CPL to 0, but it does NOT:
 *
 *     - switch RSP
 *     - save the old RSP
 *     - save general-purpose registers
 *
 * Therefore we have to do that ourselves.
 *
 * Userspace ABI:
 *
 *     RAX = syscall number
 *     RDI = argument 0
 *     RSI = argument 1
 *     RDX = argument 2
 *     R10 = argument 3
 *     R8  = argument 4
 *     R9  = argument 5
 *
 * Return value:
 *
 *     RAX
 */
global_asm!(
    r#"
    .intel_syntax noprefix

    .globl oxenna_syscall_entry
    .type oxenna_syscall_entry, @function

oxenna_syscall_entry:

    /*
     * ------------------------------------------------------------
     * Save userspace RSP
     * ------------------------------------------------------------
     *
     * SYSCALL leaves RSP unchanged.
     *
     * We MUST save it before loading the kernel stack.
     */
    mov QWORD PTR [{user_rsp}], rsp

    /*
     * ------------------------------------------------------------
     * Switch to kernel stack
     * ------------------------------------------------------------
     *
     * SYSCALL does not use TSS.RSP0.
     */
    mov rsp, QWORD PTR [{kernel_stack}]

    /*
     * ------------------------------------------------------------
     * Save all general-purpose registers
     * ------------------------------------------------------------
     *
     * Push order must match SyscallFrame.
     *
     * Current stack:
     *
     *     rsp -> r15
     *             r14
     *             ...
     *             rax
     */

    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8

    push rbp
    push rdi
    push rsi
    push rdx
    push rcx
    push rbx
    push rax

    /*
     * We now have 15 * 8 = 120 bytes on the stack.
     *
     * The stack must be 16-byte aligned before calling a Rust
     * function.
     *
     * Reserve another 8 bytes for alignment.
     */
    sub rsp, 8

    /*
     * ------------------------------------------------------------
     * Call Rust syscall dispatcher
     * ------------------------------------------------------------
     *
     * The saved RAX is 8 bytes above the current RSP because
     * of the alignment word.
     *
     * SyscallFrame therefore starts at:
     *
     *     rsp + 8
     */
    lea rdi, [rsp + 8]

    call {dispatch}

    /*
     * ------------------------------------------------------------
     * Store syscall return value
     * ------------------------------------------------------------
     *
     * syscall_dispatch returns its result in RAX.
     *
     * Save that into the saved RAX slot so that restoring the
     * registers puts the return value back into RAX.
     */
    mov QWORD PTR [rsp + 8], rax

    /*
     * Remove alignment padding.
     */
    add rsp, 8

    /*
     * ------------------------------------------------------------
     * Restore registers
     * ------------------------------------------------------------
     */

    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rbp

    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15

    /*
     * ------------------------------------------------------------
     * Restore userspace RSP
     * ------------------------------------------------------------
     */
    mov rsp, QWORD PTR [{user_rsp}]

    /*
     * ------------------------------------------------------------
     * Return to userspace
     * ------------------------------------------------------------
     *
     * SYSCALL saved:
     *
     *     RCX = user RIP
     *     R11 = user RFLAGS
     *
     * SYSRETQ consumes those registers.
     *
     * CS and SS are obtained from IA32_STAR.
     */
    sysretq

    .size oxenna_syscall_entry, .-oxenna_syscall_entry

    .att_syntax
    "#,
    user_rsp = sym SYSCALL_USER_RSP,
    kernel_stack = sym crate::gdt::SYSCALL_KERNEL_STACK_TOP,
    dispatch = sym syscall_dispatch,
);

/// Rust side of the syscall ABI.
///
/// # Safety
///
/// The pointer is created by the syscall entry assembly and points
/// into the kernel's syscall stack.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(
    frame: *mut SyscallFrame,
) -> u64 {
    let frame = unsafe {
        &mut *frame
    };

    match frame.rax {
        SYS_EXIT => {
            syscall_exit(frame.rdi)
        }

        SYS_WRITE => {
            syscall_write(
                frame.rdi,
                frame.rsi,
                frame.rdx,
            )
        }

        SYS_YIELD => {
            syscall_yield()
        }

        number => {
            crate::serial_println!(
                "[SYSCALL] unknown syscall {}",
                number
            );

            ENOSYS
        }
    }
}

/// SYS_EXIT(status)
///
/// Process management doesn't exist yet, so this currently just
/// reports the requested exit status.
///
/// Later this should never return; it should terminate the current
/// process and schedule another process.
fn syscall_exit(status: u64) -> u64 {
    crate::serial_println!(
        "[SYSCALL] exit({})",
        status
    );

    0
}

/// SYS_WRITE(fd, buffer, length)
///
/// IMPORTANT:
///
/// `buffer` is a userspace virtual address. Do NOT dereference it
/// directly from kernel code.
///
/// User-memory validation/copying should be implemented before
/// making this syscall functional.
fn syscall_write(
    fd: u64,
    buffer: u64,
    length: u64,
) -> u64 {
    crate::serial_println!(
        "[SYSCALL] write(fd={}, buffer={:#018x}, len={})",
        fd,
        buffer,
        length
    );

    match fd {
        STDOUT | STDERR => ENOSYS,
        _ => ENOSYS,
    }
}

/// SYS_YIELD()
///
/// There is no scheduler yet, so this is currently just a
/// scheduling point with no actual context switch.
fn syscall_yield() -> u64 {
    core::hint::spin_loop();

    0
}
