//! Userspace process execution and virtual-memory support.
//!
//! Oxenna executes ordinary x86_64 ELF64 files stored on ext2.  The `.ox`
//! suffix is intentionally not part of the binary format: a `.ox` file is
//! simply an ELF file with a different filename extension.

#![allow(unused)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::arch::global_asm;
use spin::Mutex;

use x86_64::{
    VirtAddr,
    structures::paging::{
        FrameAllocator, FrameDeallocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
};

use crate::{
    elf::{Elf64, ElfError, ET_DYN, PF_W, PF_X},
    fs::FS,
    gdt,
    kmem::{FRAME_ALLOCATOR, MAPPER},
    serial_println,
    syscall::{EXEC_RETURN_RSP, EXIT_REQUESTED},
};

pub const PAGE_SIZE: u64 = 4096;

// Applications are loaded into a dedicated low userspace region. `.ox`
// applications are built as ET_DYN/PIE so their ELF virtual addresses can be
// rebased here without requiring absolute-address relocations.
pub const PIE_BASE: u64 = 0x0000_0040_0000_0000;
pub const USER_STACK_TOP: u64 = 0x0000_7fff_fffe_0000;
const USER_STACK_PAGES: u64 = 16;
const HEAP_BASE: u64 = 0x0000_1000_0000_0000;
const MMAP_BASE: u64 = 0x0000_2000_0000_0000;
const USER_MAX: u64 = 0x0000_8000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserError {
    Elf(ElfError),
    Fs,
    OutOfMemory,
    Mapping,
    InvalidAddress,
    AlreadyMapped,
    StackOverflow,
    Enter,
}

impl From<ElfError> for UserError {
    fn from(e: ElfError) -> Self {
        UserError::Elf(e)
    }
}

/// Pages belonging to the current process.  Oxenna is currently single
/// process/single core, so one global address space is sufficient.
static PROCESS_PAGES: Mutex<Vec<Page<Size4KiB>>> = Mutex::new(Vec::new());
static BRK: Mutex<u64> = Mutex::new(HEAP_BASE);
static MMAP_NEXT: Mutex<u64> = Mutex::new(MMAP_BASE);

#[derive(Clone, Copy)]
struct PageSpec {
    page: Page<Size4KiB>,
    flags: PageTableFlags,
}

#[derive(Debug)]
pub struct ExecResult {
    pub status: i64,
}

pub fn exec(path: &str, argv: &[&str]) -> Result<ExecResult, UserError> {
    if !path.ends_with(".ox") {
        return Err(UserError::InvalidAddress);
    }

    let image = {
        let mut fs = FS.lock();
        fs.read_file(path).map_err(|_| UserError::Fs)?
    };

    let elf = Elf64::parse(&image)?;
    let headers = elf.load_headers(&image)?;

    let load_bias = if elf.kind == ET_DYN { PIE_BASE } else { 0 };

    let mut specs: Vec<PageSpec> = Vec::new();

    for ph in headers.iter().copied() {
        let start = ph.vaddr.checked_add(load_bias).ok_or(UserError::InvalidAddress)?;
        let end = start.checked_add(ph.memsz).ok_or(UserError::InvalidAddress)?;
        if start >= USER_MAX || end > USER_MAX || end <= start {
            return Err(UserError::InvalidAddress);
        }

        let first = align_down(start);
        let last = align_up(end);
        let mut addr = first;
        while addr < last {
            let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
            let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            if ph.flags & PF_W != 0 {
                flags |= PageTableFlags::WRITABLE;
            }
            if ph.flags & PF_X == 0 {
                flags |= PageTableFlags::NO_EXECUTE;
            }

            if let Some(existing) = specs.iter_mut().find(|s| s.page == page) {
                // ELF PT_LOAD segments are normally page-aligned. If they do
                // share a page, use the union of the required permissions.
                existing.flags |= flags;
            } else {
                specs.push(PageSpec { page, flags });
            }
            addr += PAGE_SIZE;
        }
    }

    // Build the user stack before taking the MMU locks so all allocations can
    // be recorded together.
    let stack_first = USER_STACK_TOP
        .checked_sub(USER_STACK_PAGES * PAGE_SIZE)
        .ok_or(UserError::StackOverflow)?;

    for i in 0..USER_STACK_PAGES {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(stack_first + i * PAGE_SIZE));
        if specs.iter().any(|s| s.page == page) {
            return Err(UserError::InvalidAddress);
        }
        specs.push(PageSpec {
            page,
            flags: PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE
                | PageTableFlags::NO_EXECUTE,
        });
    }

    let mapping_result: Result<(), UserError> = (|| {
        let mut mapper_guard = MAPPER.lock();
        let mut allocator_guard = FRAME_ALLOCATOR.lock();
        let mapper = mapper_guard.as_mut().ok_or(UserError::Mapping)?;
        let allocator = allocator_guard.as_mut().ok_or(UserError::OutOfMemory)?;

        for spec in specs.iter().copied() {
            if mapper.translate_page(spec.page).is_ok() {
                return Err(UserError::AlreadyMapped);
            }

            let frame = allocator.allocate_frame().ok_or(UserError::OutOfMemory)?;
            let dst = unsafe {
                crate::kmem::phys_to_ptr::<u8>(frame.start_address().as_u64())
            };
            unsafe { core::ptr::write_bytes(dst, 0, PAGE_SIZE as usize); }

            unsafe {
                if let Err(e) = mapper.map_to(spec.page, frame, spec.flags, allocator) {
                    serial_println!(
                        "ELF: map_to failed for page {:#x}: {:?}",
                        spec.page.start_address().as_u64(),
                        e
                    );
                    unsafe { allocator.deallocate_frame(frame); }
                    return Err(UserError::Mapping);
                }
                mapper
                    .translate_page(spec.page)
                    .map_err(|e| {
                        serial_println!(
                            "ELF: mapped page {:#x} cannot be translated: {:?}",
                            spec.page.start_address().as_u64(),
                            e
                        );
                        UserError::Mapping
                    })?;
            }

            PROCESS_PAGES.lock().push(spec.page);
        }

        for ph in headers.iter().copied() {
            let seg_start = ph.vaddr + load_bias;
            let src_start = ph.offset as usize;
            let src_end = src_start + ph.filesz as usize;
            let src = &image[src_start..src_end];

            let mut copied = 0usize;
            while copied < src.len() {
                let va = seg_start + copied as u64;
                let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(va));
                let phys = mapper
                    .translate_page(page)
                    .map_err(|_| UserError::Mapping)?
                    .start_address()
                    .as_u64();
                let page_off = (va & 0xfff) as usize;
                let n = core::cmp::min(4096 - page_off, src.len() - copied);

                let dst = unsafe {
                    crate::kmem::phys_to_ptr::<u8>(phys).add(page_off)
                };
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr().add(copied), dst, n);
                }
                copied += n;
            }
        }

        Ok(())
    })();

    if let Err(e) = mapping_result {
        cleanup_process();
        return Err(e);
    }

    // Construct a Linux-like initial stack:
    //
    //   argc
    //   argv[0..argc]
    //   NULL
    //   strings
    //
    // envp is deliberately empty for now.
    let mut sp = USER_STACK_TOP as usize;
    let mut arg_ptrs = Vec::new();

    let mut all_args = Vec::with_capacity(argv.len() + 1);
    all_args.push(path);
    all_args.extend_from_slice(argv);

    for arg in all_args.iter().rev() {
        let bytes = arg.as_bytes();
        sp = sp.checked_sub(bytes.len() + 1).ok_or(UserError::StackOverflow)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                sp as *mut u8,
                bytes.len(),
            );
            (sp as *mut u8).add(bytes.len()).write(0);
        }
        arg_ptrs.push(sp as u64);
    }

    arg_ptrs.reverse();

    sp &= !0xf;

    // [argc][argv[0]..argv[argc-1]][NULL][envp[0]=NULL]
    sp = sp.checked_sub(8).ok_or(UserError::StackOverflow)?;
    unsafe { (sp as *mut u64).write(0); } // envp[0]

    sp = sp.checked_sub(8).ok_or(UserError::StackOverflow)?;
    unsafe { (sp as *mut u64).write(0); } // argv[argc]

    for ptr in arg_ptrs.iter().rev() {
        sp = sp.checked_sub(8).ok_or(UserError::StackOverflow)?;
        unsafe { (sp as *mut u64).write(*ptr); }
    }

    sp = sp.checked_sub(8).ok_or(UserError::StackOverflow)?;
    unsafe { (sp as *mut u64).write(all_args.len() as u64); }

    let entry = elf.entry.checked_add(load_bias).ok_or(UserError::InvalidAddress)?;
    if !specs.iter().any(|s| {
        let start = s.page.start_address().as_u64();
        entry >= start && entry < start + PAGE_SIZE
    }) {
        cleanup_process();
        return Err(UserError::InvalidAddress);
    }

    unsafe { EXIT_REQUESTED = 0; }
    unsafe { EXEC_RETURN_RSP = 0; }

    let status = enter_user_and_wait(entry, sp as u64).map_err(|_| UserError::Enter)?;

    cleanup_process();
    unsafe { EXIT_REQUESTED = 0; }

    Ok(ExecResult { status })
}


/// Boot a legacy init program when the interactive shell is disabled.
pub fn run_init() -> ! {
    match exec("/boot/init.ox", &[]) {
        Ok(result) => panic!("init.ox exited with status {}", result.status),
        Err(e) => panic!("failed to execute /boot/init.ox: {:?}", e),
    }
}

/// Map anonymous userspace memory for brk/mmap.
pub fn map_anonymous(start: u64, len: u64, writable: bool, executable: bool) -> Result<u64, UserError> {
    if len == 0 {
        return Ok(start);
    }
    let first = align_down(start);
    let end = align_up(start.checked_add(len).ok_or(UserError::InvalidAddress)?);
    if first >= USER_MAX || end > USER_MAX || end <= first {
        return Err(UserError::InvalidAddress);
    }

    let mut mapper_guard = MAPPER.lock();
    let mut allocator_guard = FRAME_ALLOCATOR.lock();
    let mapper = mapper_guard.as_mut().ok_or(UserError::Mapping)?;
    let allocator = allocator_guard.as_mut().ok_or(UserError::OutOfMemory)?;

    let mut addr = first;
    while addr < end {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
        if mapper.translate_page(page).is_ok() {
            addr += PAGE_SIZE;
            continue;
        }

        let frame = allocator.allocate_frame().ok_or(UserError::OutOfMemory)?;
        let ptr = unsafe {
            crate::kmem::phys_to_ptr::<u8>(frame.start_address().as_u64())
        };
        unsafe { core::ptr::write_bytes(ptr, 0, 4096); }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable { flags |= PageTableFlags::WRITABLE; }
        if !executable { flags |= PageTableFlags::NO_EXECUTE; }

        unsafe {
            mapper
                .map_to(page, frame, flags, allocator)
                .map_err(|_| UserError::Mapping)?
                .flush();
        }
        PROCESS_PAGES.lock().push(page);
        addr += PAGE_SIZE;
    }

    Ok(start)
}

pub fn current_brk() -> u64 {
    *BRK.lock()
}

pub fn set_brk(new_brk: u64) -> Result<u64, UserError> {
    let old = *BRK.lock();
    if new_brk <= old {
        *BRK.lock() = new_brk;
        return Ok(new_brk);
    }

    map_anonymous(old, new_brk - old, true, false)?;
    *BRK.lock() = new_brk;
    Ok(new_brk)
}

pub fn mprotect(start: u64, len: u64, writable: bool, executable: bool) -> Result<(), UserError> {
    if len == 0 {
        return Err(UserError::InvalidAddress);
    }
    let first = align_down(start);
    let end = align_up(start.checked_add(len).ok_or(UserError::InvalidAddress)?);

    let mut mapper_guard = MAPPER.lock();
    let mapper = mapper_guard.as_mut().ok_or(UserError::Mapping)?;
    let mut addr = first;
    while addr < end {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
        if mapper.translate_page(page).is_err() {
            return Err(UserError::InvalidAddress);
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable { flags |= PageTableFlags::WRITABLE; }
        if !executable { flags |= PageTableFlags::NO_EXECUTE; }

        unsafe {
            mapper
                .update_flags(page, flags)
                .map_err(|_| UserError::Mapping)?
                .flush();
        }
        addr += PAGE_SIZE;
    }
    Ok(())
}

pub fn munmap(start: u64, len: u64) -> Result<(), UserError> {
    if len == 0 {
        return Err(UserError::InvalidAddress);
    }

    let first = align_down(start);
    let end = align_up(start.checked_add(len).ok_or(UserError::InvalidAddress)?);
    let mut mapper_guard = MAPPER.lock();
    let mut allocator_guard = FRAME_ALLOCATOR.lock();
    let mapper = mapper_guard.as_mut().ok_or(UserError::Mapping)?;
    let allocator = allocator_guard.as_mut().ok_or(UserError::OutOfMemory)?;

    let mut pages = PROCESS_PAGES.lock();
    let mut addr = first;

    while addr < end {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
        if let Some(pos) = pages.iter().position(|p| *p == page) {
            if let Ok((frame, flush)) = unsafe { mapper.unmap(page) } {
                flush.flush();
                unsafe { allocator.deallocate_frame(frame); }
                pages.swap_remove(pos);
            }
        }
        addr += PAGE_SIZE;
    }

    Ok(())
}

pub fn mmap_anon(len: u64, writable: bool, executable: bool) -> Result<u64, UserError> {
    let mut next = MMAP_NEXT.lock();
    let addr = align_up(*next);
    let end = addr.checked_add(len).ok_or(UserError::InvalidAddress)?;
    *next = align_up(end);
    drop(next);
    map_anonymous(addr, len, writable, executable)
}

pub fn cleanup_process() {
    let mut pages = PROCESS_PAGES.lock();
    let mut mapper_guard = MAPPER.lock();
    let mut allocator_guard = FRAME_ALLOCATOR.lock();

    let Some(mapper) = mapper_guard.as_mut() else { return; };
    let Some(allocator) = allocator_guard.as_mut() else { return; };

    while let Some(page) = pages.pop() {
        if let Ok((frame, flush)) = unsafe { mapper.unmap(page) } {
            flush.flush();
            unsafe { allocator.deallocate_frame(frame); }
        }
    }

    *BRK.lock() = HEAP_BASE;
    *MMAP_NEXT.lock() = MMAP_BASE;
    crate::syscall::reset_process_fds();
}

fn align_down(v: u64) -> u64 {
    v & !0xfff
}
fn align_up(v: u64) -> u64 {
    (v + 0xfff) & !0xfff
}

#[derive(Debug)]
enum EnterError {
    Failed,
}

unsafe extern "C" {
    fn oxenna_enter_user_and_wait(
        entry: u64,
        stack: u64,
        user_cs: u64,
        user_ss: u64,
    ) -> u64;
}

fn enter_user_and_wait(entry: u64, stack: u64) -> Result<i64, EnterError> {
    let selectors = gdt::selectors();
    let user_cs = ((selectors.user_code_selector.index() as u64) << 3) | 3;
    let user_ss = ((selectors.user_data_selector.index() as u64) << 3) | 3;

    let status = unsafe {
        oxenna_enter_user_and_wait(entry, stack, user_cs, user_ss)
    };
    Ok(status as i64)
}

global_asm!(
r#"
    .globl oxenna_enter_user_and_wait
    .type oxenna_enter_user_and_wait, @function

oxenna_enter_user_and_wait:
    /*
     * Save the kernel stack containing this function's return address.
     * syscall_exit will switch back here and `ret` into the caller.
     */
    mov QWORD PTR [{return_rsp}], rsp

    /*
     * Enter ring 3 using an iret frame.
     */
    push rcx
    push rsi
    mov rax, 0x202
    push rax
    push rdx
    push rdi
    iretq

    .size oxenna_enter_user_and_wait, .-oxenna_enter_user_and_wait
"#,
    return_rsp = sym EXEC_RETURN_RSP,
);
