#![allow(unused)]
use core::arch::asm;

use limine::request::ModulesRequest;

use x86_64::{
    VirtAddr,
    structures::paging::{
        FrameAllocator,
        Mapper,
        Page,
        PageTableFlags,
        Size4KiB,
    },
};

use crate::{
    gdt,
    kmem::{
        FRAME_ALLOCATOR,
        MAPPER,
    },
    serial_println,
};

const USER_BASE: u64 =
    0x0000_4000_0000_0000;

const USER_STACK_TOP: u64 =
    0x0000_4000_0010_0000;

const USER_STACK_PAGES: u64 =
    4;

const USER_MODULE_PATH: &str =
    "/boot/user.bin";

#[used]
#[unsafe(link_section = ".limine_reqs")]
pub static MODULES_REQUEST:
    ModulesRequest =
    ModulesRequest::new();

pub fn run() -> ! {
    let module =
        find_user_module();

    let image =
        module.data();

    serial_println!(
        "[USER] module: {}",
        module.path()
    );

    serial_println!(
        "[USER] size: {} bytes",
        image.len()
    );

    assert!(
        !image.is_empty(),
        "user binary is empty"
    );

    let mut mapper_guard =
        MAPPER.lock();

    let mut frame_allocator_guard =
        FRAME_ALLOCATOR.lock();

    let mapper =
        mapper_guard
            .as_mut()
            .expect(
                "mapper not initialized"
            );

    let frame_allocator =
        frame_allocator_guard
            .as_mut()
            .expect(
                "frame allocator not initialized"
            );

    let entry =
        map_user_image(
            mapper,
            frame_allocator,
            image,
        );

    let user_stack =
        map_user_stack(
            mapper,
            frame_allocator,
        );

    serial_println!(
        "[USER] entry = {:#018x}",
        entry
    );

    serial_println!(
        "[USER] stack = {:#018x}",
        user_stack
    );

    /*
     * We don't want to keep the memory-manager locks
     * held while executing userspace.
     */
    drop(mapper_guard);
    drop(frame_allocator_guard);

    enter_user_mode(
        entry,
        user_stack,
    );
}

fn find_user_module()
    -> &'static limine::file::File
{
    let response =
        MODULES_REQUEST
            .response()
            .expect(
                "Limine did not provide module response"
            );

    for module in response.modules() {
        if module.path() ==
            USER_MODULE_PATH
        {
            return module;
        }
    }

    panic!(
        "user module not found: {}",
        USER_MODULE_PATH
    );
}

fn map_user_image(
    mapper:
        &mut impl Mapper<Size4KiB>,

    frame_allocator:
        &mut impl FrameAllocator<Size4KiB>,

    image: &[u8],
) -> u64 {
    let page_count =
        (image.len() as u64 + 4095) / 4096;

    /*
     * User code:
     *
     * PRESENT
     * USER_ACCESSIBLE
     *
     * No WRITABLE:
     *   user code is read-only.
     *
     * No NO_EXECUTE:
     *   user code is executable.
     */
    let flags =
        PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE;

    for page_index in
        0..page_count
    {
        let virtual_address =
            USER_BASE
                + page_index * 4096;

        let page =
            Page::<Size4KiB>
                ::containing_address(
                    VirtAddr::new(
                        virtual_address
                    )
                );

        let frame =
            frame_allocator
                .allocate_frame()
                .expect(
                    "out of physical memory"
                );

        unsafe {
            mapper
                .map_to(
                    page,
                    frame,
                    flags,
                    frame_allocator,
                )
                .expect(
                    "failed to map user code"
                )
                .flush();
        }

        /*
         * Copy the user binary into the
         * physical frame through the HHDM.
         */
        let page_offset =
            (page_index * 4096) as usize;

        let remaining =
            image
                .len()
                .saturating_sub(
                    page_offset
                );

        let copy_len =
            remaining.min(4096);

        let destination =
            unsafe {
                crate::kmem::phys_to_ptr::<u8>(
                    frame
                        .start_address()
                        .as_u64()
                )
            };

        unsafe {
            core::ptr::copy_nonoverlapping(
                image
                    .as_ptr()
                    .add(page_offset),
                destination,
                copy_len,
            );
        }

        /*
         * Zero the remainder of the final
         * page.
         */
        if copy_len < 4096 {
            unsafe {
                core::ptr::write_bytes(
                    destination.add(copy_len),
                    0,
                    4096 - copy_len,
                );
            }
        }
    }

    USER_BASE
}

fn map_user_stack(
    mapper:
        &mut impl Mapper<Size4KiB>,

    frame_allocator:
        &mut impl FrameAllocator<Size4KiB>,
) -> u64 {
    let flags =
        PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    let first_page =
        Page::<Size4KiB>
            ::containing_address(
                VirtAddr::new(
                    USER_STACK_TOP
                        - USER_STACK_PAGES
                            * 4096
                )
            );

    let last_page =
        Page::<Size4KiB>
            ::containing_address(
                VirtAddr::new(
                    USER_STACK_TOP - 1
                )
            );

    for page in
        Page::range_inclusive(
            first_page,
            last_page,
        )
    {
        let frame =
            frame_allocator
                .allocate_frame()
                .expect(
                    "out of physical memory"
                );

        unsafe {
            mapper
                .map_to(
                    page,
                    frame,
                    flags,
                    frame_allocator,
                )
                .expect(
                    "failed to map user stack"
                )
                .flush();
        }

        /*
         * Zero the stack.
         */
        let ptr =
            unsafe {
                crate::kmem::phys_to_ptr::<u8>(
                    frame
                        .start_address()
                        .as_u64()
                )
            };

        unsafe {
            core::ptr::write_bytes(
                ptr,
                0,
                4096,
            );
        }
    }

    USER_STACK_TOP
}

fn enter_user_mode(
    entry: u64,
    stack: u64,
) -> ! {
    let selectors =
        gdt::selectors();

    let user_cs =
        (selectors
            .user_code_selector
            .index() as u64)
            << 3
            | 3;

    let user_ss =
        (selectors
            .user_data_selector
            .index() as u64)
            << 3
            | 3;

    /*
     * iretq expects this frame:
     *
     *     SS
     *     RSP
     *     RFLAGS
     *     CS
     *     RIP
     *
     * with RIP at the top of the stack.
     */

    let rflags =
        unsafe { read_rflags() };

    unsafe {
        asm!(
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",

            ss = in(reg) user_ss,
            rsp = in(reg) stack,
            rflags = in(reg) rflags,
            cs = in(reg) user_cs,
            rip = in(reg) entry,

            options(noreturn)
        );
    }
}

#[inline(always)]
unsafe fn read_rflags() -> u64 {
    let flags: u64;

    unsafe {
        asm!(
            "pushfq",
            "pop {0}",
            out(reg) flags,
            options(nomem, preserves_flags)
        );
    }

    flags
}
