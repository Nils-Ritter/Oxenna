//! Linux-shaped x86-64 syscall ABI used by `.ox` programs.
//!
//! The ABI intentionally follows Linux syscall numbers where practical.  This
//! makes it possible to write tiny programs in C/assembly that use `syscall`
//! directly, while keeping the implementation small enough for the current
//! single-process kernel.

use core::arch::global_asm;
use alloc::{string::String, vec, vec::Vec};
use spin::Mutex;
use x86_64::structures::paging::{Mapper, Page, Size4KiB};

use crate::console;
use crate::fs::{Entry, FileType, FS};

#[cfg(feature = "userspace")]
use crate::user;

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_READLINK: u64 = 89;
pub const SYS_UNAME: u64 = 63;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_EXIT: u64 = 60;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_RSEQ: u64 = 334;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_GETUID: u64 = 102;
pub const SYS_GETGID: u64 = 104;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_YIELD: u64 = 24;

pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;
pub const STDERR: u64 = 2;

const AT_FDCWD: i64 = -100;
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;
const O_CREAT: u64 = 64;
const O_EXCL: u64 = 128;
const O_TRUNC: u64 = 512;
const O_APPEND: u64 = 1024;
const O_DIRECTORY: u64 = 0x10000;

const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;
const MAP_ANON: u64 = 0x20;

const EFAULT: i64 = 14;
const ENOENT: i64 = 2;
const EBADF: i64 = 9;
const EINVAL: i64 = 22;
const EEXIST: i64 = 17;
const EISDIR: i64 = 21;
const ENOTDIR: i64 = 20;
const ENOSYS_I: i64 = 38;
const ENOTTY: i64 = 25;
const EAGAIN: i64 = 11;
const ENOEXEC: i64 = 8;

pub const ENOSYS: u64 = (-ENOSYS_I) as u64;

/// Set to one by SYS_exit/exit_group. The syscall assembly uses this to
/// return from `user::exec()` instead of executing SYSRET back into dead code.
#[unsafe(no_mangle)]
pub static mut EXIT_REQUESTED: u8 = 0;

#[unsafe(no_mangle)]
pub static mut EXIT_STATUS: u64 = 0;

/// Saved kernel RSP for the `user::exec()` continuation.
#[unsafe(no_mangle)]
pub static mut EXEC_RETURN_RSP: u64 = 0;

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

unsafe extern "C" {
    pub fn oxenna_syscall_entry();
}

pub fn syscall_entry_address() -> x86_64::VirtAddr {
    x86_64::VirtAddr::from_ptr(oxenna_syscall_entry as *const ())
}

global_asm!(
r#"
    .globl oxenna_syscall_entry
    .type oxenna_syscall_entry, @function

oxenna_syscall_entry:
    mov QWORD PTR [{user_rsp}], rsp
    mov rsp, QWORD PTR [{kernel_stack}]

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

    sub rsp, 8
    lea rdi, [rsp + 8]
    call {dispatch}

    cmp BYTE PTR [{exit_requested}], 0
    jne .exit_to_kernel

    mov QWORD PTR [rsp + 8], rax
    add rsp, 8

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

    mov rsp, QWORD PTR [{user_rsp}]
    sysretq

.exit_to_kernel:
    mov rax, QWORD PTR [{exit_status}]
    mov rsp, QWORD PTR [{exec_return_rsp}]
    ret

    .size oxenna_syscall_entry, .-oxenna_syscall_entry
"#,
    user_rsp = sym SYSCALL_USER_RSP,
    kernel_stack = sym crate::gdt::SYSCALL_KERNEL_STACK_TOP,
    dispatch = sym syscall_dispatch,
    exit_requested = sym EXIT_REQUESTED,
    exit_status = sym EXIT_STATUS,
    exec_return_rsp = sym EXEC_RETURN_RSP,
);

#[unsafe(no_mangle)]
pub static mut SYSCALL_USER_RSP: u64 = 0;

#[derive(Clone)]
struct OpenFile {
    path: String,
    offset: u64,
    flags: u64,
}

static FD_TABLE: Mutex<Option<Vec<Option<OpenFile>>>> = Mutex::new(None);

pub fn reset_process_fds() {
    let mut guard = fds();
    let table = guard.as_mut().unwrap();
    table.truncate(3);
}

fn fds() -> spin::MutexGuard<'static, Option<Vec<Option<OpenFile>>>> {
    let mut guard = FD_TABLE.lock();
    if guard.is_none() {
        let mut v = Vec::new();
        v.push(Some(OpenFile { path: String::new(), offset: 0, flags: 0 }));
        v.push(Some(OpenFile { path: String::new(), offset: 0, flags: 0 }));
        v.push(Some(OpenFile { path: String::new(), offset: 0, flags: 0 }));
        *guard = Some(v);
    }
    guard
}

fn ret(v: i64) -> u64 { v as u64 }
fn err(e: i64) -> u64 { (-e) as u64 }

fn user_range_ok(ptr: u64, len: usize) -> bool {
    if len == 0 { return ptr < 0x0000_8000_0000_0000; }
    let end = match ptr.checked_add(len as u64 - 1) { Some(v) => v, None => return false };
    if ptr >= 0x0000_8000_0000_0000 || end >= 0x0000_8000_0000_0000 {
        return false;
    }

    let mut mapper = crate::kmem::MAPPER.lock();
    let Some(mapper) = mapper.as_mut() else { return false; };

    let first = ptr & !0xfff;
    let last = end & !0xfff;
    let mut page = first;
    loop {
        if mapper.translate_page(Page::<Size4KiB>::containing_address(
            x86_64::VirtAddr::new(page)
        )).is_err() {
            return false;
        }
        if page == last { break; }
        page += 4096;
    }
    true
}

fn copy_from_user(ptr: u64, len: usize) -> Result<Vec<u8>, u64> {
    if len > 16 * 1024 * 1024 || !user_range_ok(ptr, len) {
        return Err(err(EFAULT));
    }
    let mut out = vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), len);
    }
    Ok(out)
}

fn copy_to_user(ptr: u64, data: &[u8]) -> Result<(), u64> {
    if !user_range_ok(ptr, data.len()) {
        return Err(err(EFAULT));
    }
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    }
    Ok(())
}

fn user_string(ptr: u64, max: usize) -> Result<String, u64> {
    if ptr == 0 { return Err(err(EFAULT)); }
    let mut bytes = Vec::new();
    for i in 0..max {
        if !user_range_ok(ptr + i as u64, 1) {
            return Err(err(EFAULT));
        }
        let c = unsafe { *(ptr as *const u8).add(i) };
        if c == 0 {
            return String::from_utf8(bytes).map_err(|_| err(EFAULT));
        }
        bytes.push(c);
    }
    Err(err(EFAULT))
}

#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) -> u64 {
    let frame = unsafe { &mut *frame };

    match frame.rax {
        SYS_READ => syscall_read(frame.rdi, frame.rsi, frame.rdx),
        SYS_WRITE => syscall_write(frame.rdi, frame.rsi, frame.rdx),
        SYS_OPEN => syscall_open(frame.rdi, frame.rsi, frame.rdx),
        SYS_CLOSE => syscall_close(frame.rdi),
        SYS_STAT => syscall_stat(frame.rdi, frame.rsi),
        SYS_FSTAT => syscall_fstat(frame.rdi, frame.rsi),
        SYS_LSEEK => syscall_lseek(frame.rdi, frame.rsi as i64, frame.rdx),
        SYS_MMAP => syscall_mmap(frame),
        SYS_MPROTECT => syscall_mprotect(frame.rdi, frame.rsi, frame.rdx),
        SYS_MUNMAP => syscall_munmap(frame.rdi, frame.rsi),
        SYS_BRK => syscall_brk(frame.rdi),
        SYS_RT_SIGACTION | SYS_RT_SIGPROCMASK => ret(0),
        SYS_IOCTL => err(ENOTTY),
        SYS_ACCESS => syscall_access(frame.rdi),
        SYS_DUP => syscall_dup(frame.rdi),
        SYS_DUP2 => syscall_dup2(frame.rdi, frame.rsi),
        SYS_NANOSLEEP => ret(0),
        SYS_GETPID => ret(1),
        SYS_GETUID | SYS_GETGID | SYS_GETEUID | SYS_GETEGID => ret(0),
        SYS_GETCWD => syscall_getcwd(frame.rdi, frame.rsi),
        SYS_CHDIR => syscall_chdir(frame.rdi),
        SYS_READLINK => syscall_readlink(frame.rdi, frame.rsi, frame.rdx),
        SYS_UNAME => syscall_uname(frame.rdi),
        SYS_GETDENTS64 => syscall_getdents64(frame.rdi, frame.rsi, frame.rdx),
        SYS_CLOCK_GETTIME => syscall_clock_gettime(frame.rsi),
        SYS_FUTEX => syscall_futex(frame),
        SYS_SET_TID_ADDRESS => ret(1),
        SYS_SET_ROBUST_LIST | SYS_RSEQ | SYS_PRLIMIT64 | SYS_PRCTL => ret(0),
        SYS_GETRANDOM => syscall_getrandom(frame.rdi, frame.rsi),
        SYS_ARCH_PRCTL => syscall_arch_prctl(frame.rdi, frame.rsi),
        SYS_OPENAT => syscall_openat(frame.rdi, frame.rsi, frame.rdx, frame.r10),
        SYS_NEWFSTATAT => syscall_newfstatat(frame.rdi, frame.rsi, frame.rdx, frame.r10),
        SYS_EXIT | SYS_EXIT_GROUP => syscall_exit(frame.rdi),
        SYS_YIELD => { core::hint::spin_loop(); ret(0) }
        _ => err(ENOSYS_I),
    }
}

fn syscall_exit(status: u64) -> u64 {
    unsafe {
        EXIT_STATUS = status;
        EXIT_REQUESTED = 1;
    }
    0
}

fn syscall_write(fd: u64, ptr: u64, len: u64) -> u64 {
    let data = match copy_from_user(ptr, len as usize) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match fd {
        STDOUT | STDERR => {
            let guard = fds();
            let table = guard.as_ref().unwrap();
            if table.get(fd as usize).and_then(|x| x.as_ref()).is_none() {
                return err(EBADF);
            }
            drop(guard);

            let text = String::from_utf8_lossy(&data);
            crate::console_print!("{}", text);
            data.len() as u64
        }
        _ => {
            let mut guard = fds();
            let Some(table) = guard.as_mut() else { return err(EBADF); };
            let Some(Some(file)) = table.get_mut(fd as usize) else { return err(EBADF); };
            if file.flags & 3 == 0 { return err(EBADF); }

            if file.flags & O_APPEND != 0 {
                if let Ok(stat) = FS.lock().stat(&file.path) {
                    file.offset = stat.size;
                }
            }
            let n = {
                let mut fs = FS.lock();
                match fs.write_at(&file.path, file.offset, &data) {
                    Ok(n) => n,
                    Err(_) => return err(EINVAL),
                }
            };
            file.offset += n as u64;
            n as u64
        }
    }
}

fn syscall_read(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd == STDIN {
        let guard = fds();
        if guard.as_ref().unwrap().get(0).and_then(|x| x.as_ref()).is_none() {
            return err(EBADF);
        }
        return 0; // stdin is not yet connected to a blocking line discipline
    }

    let mut guard = fds();
    let Some(table) = guard.as_mut() else { return err(EBADF); };
    let Some(Some(file)) = table.get_mut(fd as usize) else { return err(EBADF); };

    let mut data = vec![0u8; core::cmp::min(len as usize, 1024 * 1024)];
    let n = {
        let mut fs = FS.lock();
        match fs.read_at(&file.path, file.offset, &mut data) {
            Ok(n) => n,
            Err(_) => return err(EINVAL),
        }
    };
    if let Err(e) = copy_to_user(ptr, &data[..n]) { return e; }
    file.offset += n as u64;
    n as u64
}

fn alloc_fd(file: OpenFile) -> u64 {
    let mut guard = fds();
    let table = guard.as_mut().unwrap();
    for (i, slot) in table.iter_mut().enumerate().skip(3) {
        if slot.is_none() {
            *slot = Some(file);
            return i as u64;
        }
    }
    table.push(Some(file));
    (table.len() - 1) as u64
}

fn syscall_open(path_ptr: u64, flags: u64, mode: u64) -> u64 {
    syscall_openat(AT_FDCWD as u64, path_ptr, flags, mode)
}

fn syscall_openat(_dirfd: u64, path_ptr: u64, flags: u64, mode: u64) -> u64 {
    let path = match user_string(path_ptr, 4096) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut fs = FS.lock();
    let stat = match fs.stat(&path) {
        Ok(s) => s,
        Err(_) if flags & O_CREAT != 0 => {
            if fs.touch(&path).is_err() {
                return err(ENOENT);
            }
            match fs.stat(&path) {
                Ok(s) => s,
                Err(_) => return err(ENOENT),
            }
        }
        Err(_) => return err(ENOENT),
    };

    if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
        // If it already existed, O_EXCL must fail.
        return err(EEXIST);
    }
    if flags & O_DIRECTORY != 0 && stat.file_type != FileType::Directory {
        return err(ENOTDIR);
    }
    if flags & O_TRUNC != 0 && stat.file_type == FileType::Regular {
        if fs.write_file(&path, &[]).is_err() {
            return err(EINVAL);
        }
    }
    drop(fs);

    alloc_fd(OpenFile {
        path,
        offset: 0,
        flags,
    })
}

fn syscall_close(fd: u64) -> u64 {
    let mut guard = fds();
    let Some(table) = guard.as_mut() else { return err(EBADF); };
    if fd as usize >= table.len() || table[fd as usize].is_none() {
        return err(EBADF);
    }
    table[fd as usize] = None;
    0
}

fn syscall_stat(path_ptr: u64, stat_ptr: u64) -> u64 {
    let path = match user_string(path_ptr, 4096) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let stat = match FS.lock().stat(&path) {
        Ok(s) => s,
        Err(_) => return err(ENOENT),
    };
    write_stat(stat_ptr, &stat)
}

fn syscall_fstat(fd: u64, stat_ptr: u64) -> u64 {
    if fd == STDOUT || fd == STDERR {
        let synthetic = crate::fs::Stat {
            ino: fd as u32,
            mode: 0o020620,
            file_type: FileType::CharDevice,
            uid: 0,
            gid: 0,
            size: 0,
            links: 1,
            blocks: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
        };
        return write_stat(stat_ptr, &synthetic);
    }

    let guard = fds();
    let table = guard.as_ref().unwrap();
    let Some(Some(file)) = table.get(fd as usize) else { return err(EBADF); };
    let stat = match FS.lock().stat(&file.path) {
        Ok(s) => s,
        Err(_) => return err(ENOENT),
    };
    write_stat(stat_ptr, &stat)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: i32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    __unused: [i64; 3],
}

fn write_stat(ptr: u64, s: &crate::fs::Stat) -> u64 {
    let out = LinuxStat {
        st_dev: 0,
        st_ino: s.ino as u64,
        st_nlink: s.links as u64,
        st_mode: s.mode as u32,
        st_uid: s.uid,
        st_gid: s.gid,
        __pad0: 0,
        st_rdev: 0,
        st_size: s.size as i64,
        st_blksize: 4096,
        st_blocks: s.blocks as i64,
        st_atime: s.atime as i64,
        st_atime_nsec: 0,
        st_mtime: s.mtime as i64,
        st_mtime_nsec: 0,
        st_ctime: s.ctime as i64,
        st_ctime_nsec: 0,
        __unused: [0; 3],
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &out as *const LinuxStat as *const u8,
            core::mem::size_of::<LinuxStat>(),
        )
    };
    match copy_to_user(ptr, bytes) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn syscall_lseek(fd: u64, off: i64, whence: u64) -> u64 {
    let mut guard = fds();
    let table = guard.as_mut().unwrap();
    let Some(Some(file)) = table.get_mut(fd as usize) else { return err(EBADF); };
    let size = match FS.lock().stat(&file.path) {
        Ok(s) => s.size as i64,
        Err(_) => return err(ENOENT),
    };

    let base = match whence {
        0 => 0,
        1 => file.offset as i64,
        2 => size,
        _ => return err(EINVAL),
    };
    let new = match base.checked_add(off) {
        Some(v) if v >= 0 => v as u64,
        _ => return err(EINVAL),
    };
    file.offset = new;
    new
}

fn syscall_access(path_ptr: u64) -> u64 {
    let path = match user_string(path_ptr, 4096) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if FS.lock().stat(&path).is_ok() { 0 } else { err(ENOENT) }
}

fn syscall_dup(fd: u64) -> u64 {
    let guard = fds();
    let table = guard.as_ref().unwrap();
    let Some(Some(file)) = table.get(fd as usize) else { return err(EBADF); };
    alloc_fd(file.clone())
}

fn syscall_dup2(fd: u64, newfd: u64) -> u64 {
    if fd == newfd { return fd; }
    let guard = fds();
    let table = guard.as_ref().unwrap();
    let Some(Some(file)) = table.get(fd as usize) else { return err(EBADF); };
    let file = file.clone();
    drop(guard);

    let mut guard = fds();
    let table = guard.as_mut().unwrap();
    while table.len() <= newfd as usize { table.push(None); }
    table[newfd as usize] = Some(file);
    newfd
}

fn syscall_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    #[cfg(feature = "userspace")]
    {
        return match user::mprotect(addr, len, prot & PROT_WRITE != 0, prot & PROT_EXEC != 0) {
            Ok(()) => 0,
            Err(_) => err(EINVAL),
        };
    }
    #[cfg(not(feature = "userspace"))]
    { err(ENOSYS_I) }
}

fn syscall_munmap(addr: u64, len: u64) -> u64 {
    #[cfg(feature = "userspace")]
    {
        return match user::munmap(addr, len) {
            Ok(()) => 0,
            Err(_) => err(EINVAL),
        };
    }
    #[cfg(not(feature = "userspace"))]
    { err(ENOSYS_I) }
}

fn syscall_mmap(frame: &SyscallFrame) -> u64 {
    // Only anonymous private mappings are currently supported.
    if frame.r10 & MAP_ANON == 0 {
        return err(ENOSYS_I);
    }
    let len = frame.rsi;
    if len == 0 { return err(EINVAL); }
    let writable = frame.rdx & PROT_WRITE != 0;
    let executable = frame.rdx & PROT_EXEC != 0;

    #[cfg(feature = "userspace")]
    {
        let addr = if frame.rdi != 0 && frame.rdi != u64::MAX {
            frame.rdi
        } else {
            0
        };
        if addr != 0 {
            return match user::map_anonymous(addr, len, writable, executable) {
                Ok(v) => v,
                Err(_) => err(EINVAL),
            };
        }
        return match user::mmap_anon(len, writable, executable) {
            Ok(v) => v,
            Err(_) => err(EINVAL),
        };
    }
    #[cfg(not(feature = "userspace"))]
    { err(ENOSYS_I) }
}

fn syscall_brk(addr: u64) -> u64 {
    #[cfg(feature = "userspace")]
    {
        if addr == 0 { return user::current_brk(); }
        return match user::set_brk(addr) {
            Ok(v) => v,
            Err(_) => user::current_brk(),
        };
    }
    #[cfg(not(feature = "userspace"))]
    { 0 }
}

fn syscall_getcwd(ptr: u64, size: u64) -> u64 {
    if size < 2 { return err(EINVAL); }
    let path = b"/\0";
    if (size as usize) < path.len() { return err(EINVAL); }
    match copy_to_user(ptr, path) {
        Ok(()) => 2,
        Err(e) => e,
    }
}

fn syscall_chdir(path_ptr: u64) -> u64 {
    let path = match user_string(path_ptr, 4096) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if FS.lock().stat(&path).map(|s| s.file_type == FileType::Directory).unwrap_or(false) {
        0
    } else {
        err(ENOENT)
    }
}

fn syscall_readlink(path_ptr: u64, buf_ptr: u64, len: u64) -> u64 {
    let path = match user_string(path_ptr, 4096) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let data = match FS.lock().readlink(&path) {
        Ok(v) => v,
        Err(_) => return err(ENOENT),
    };
    let n = core::cmp::min(data.len(), len as usize);
    match copy_to_user(buf_ptr, &data[..n]) {
        Ok(()) => n as u64,
        Err(e) => e,
    }
}

fn syscall_getrandom(ptr: u64, len: u64) -> u64 {
    let n = core::cmp::min(len as usize, 1024 * 1024);
    let data = vec![0u8; n];
    match copy_to_user(ptr, &data) {
        Ok(()) => n as u64,
        Err(e) => e,
    }
}

fn syscall_uname(ptr: u64) -> u64 {
    let mut out = [0u8; 390];
    let fields = [
        b"Oxenna\0".as_slice(),
        b"oxenna\0".as_slice(),
        b"0.1.0\0".as_slice(),
        b"1\0".as_slice(),
        b"x86_64\0".as_slice(),
        b"oxenna\0".as_slice(),
    ];
    for (i, f) in fields.iter().enumerate() {
        out[i * 65..i * 65 + f.len()].copy_from_slice(f);
    }
    match copy_to_user(ptr, &out) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn syscall_getdents64(fd: u64, ptr: u64, len: u64) -> u64 {
    let mut guard = fds();
    let table = guard.as_mut().unwrap();
    let Some(Some(file)) = table.get_mut(fd as usize) else { return err(EBADF); };

    let entries = match FS.lock().readdir(&file.path) {
        Ok(v) => v,
        Err(_) => return err(ENOTDIR),
    };

    let start = file.offset as usize;
    let mut out = Vec::new();
    let mut emitted = 0usize;

    for (idx, e) in entries.iter().enumerate().skip(start) {
        let name = e.name.as_bytes();
        let reclen = (19 + name.len() + 1 + 7) & !7;
        if out.len() + reclen > len as usize {
            break;
        }
        out.resize(out.len() + reclen, 0);
        let p = out.len() - reclen;
        out[p..p+8].copy_from_slice(&(e.ino as u64).to_le_bytes());
        out[p+8..p+16].copy_from_slice(&((idx + 1) as i64).to_le_bytes());
        out[p+16..p+18].copy_from_slice(&(reclen as u16).to_le_bytes());
        out[p+18] = match e.file_type {
            FileType::Directory => 4,
            FileType::Regular => 8,
            FileType::Symlink => 10,
            _ => 0,
        };
        out[p+19..p+19+name.len()].copy_from_slice(name);
        emitted += 1;
    }

    let count = out.len();
    if count == 0 {
        return 0;
    }
    if let Err(e) = copy_to_user(ptr, &out) { return e; }
    file.offset = start as u64 + emitted as u64;
    count as u64
}

fn syscall_clock_gettime(ptr: u64) -> u64 {
    // No wall clock source is exposed by the current kernel yet.
    let ts = [0u8; 16];
    match copy_to_user(ptr, &ts) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn syscall_futex(frame: &SyscallFrame) -> u64 {
    // Single-threaded kernel/process: a matching WAIT cannot make progress,
    // so report EAGAIN and let a userspace mutex retry.
    let op = frame.rsi & 0x7f;
    if op == 0 || op == 128 { err(EAGAIN) } else { ret(0) }
}

fn syscall_arch_prctl(code: u64, addr: u64) -> u64 {
    // Linux ARCH_SET_FS/GS.  We currently use the architectural MSRs directly.
    const ARCH_SET_GS: u64 = 0x1001;
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    const ARCH_GET_GS: u64 = 0x1004;

    match code {
        ARCH_SET_FS => unsafe { wrmsr(0xC000_0100, addr); ret(0) },
        ARCH_SET_GS => unsafe { wrmsr(0xC000_0101, addr); ret(0) },
        ARCH_GET_FS => unsafe { if copy_to_user(addr, &rdmsr(0xC000_0100).to_le_bytes()).is_ok() { 0 } else { err(EFAULT) } },
        ARCH_GET_GS => unsafe { if copy_to_user(addr, &rdmsr(0xC000_0101).to_le_bytes()).is_ok() { 0 } else { err(EFAULT) } },
        _ => err(EINVAL),
    }
}

unsafe fn wrmsr(msr: u32, value: u64) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") value as u32,
        in("edx") (value >> 32) as u32,
        options(nostack, preserves_flags)
    );
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, preserves_flags)
    );
    ((hi as u64) << 32) | lo as u64
}

fn syscall_newfstatat(_dirfd: u64, path_ptr: u64, stat_ptr: u64, _flags: u64) -> u64 {
    syscall_stat(path_ptr, stat_ptr)
}
