//! Kernel memory management.
//!
//! This module provides the foundations for kernel memory management:
//!
//! - Limine HHDM access
//! - Physical frame allocation
//! - Page-table mapping
//! - Kernel heap initialization
//!
//! The individual allocators live in their own modules.

pub mod frame;
pub mod heap;
pub mod memtests;

pub use frame::BootInfoFrameAllocator;
pub use heap::{init_heap, LockedHeap, HEAP_SIZE, HEAP_START};

use limine::request::{HhdmRequest, MemmapRequest};
use spin::Mutex;
use x86_64::{
    registers::control::Cr3,
    structures::paging::{OffsetPageTable, PageTable},
    VirtAddr,
};

use crate::console_println_color;

/// Limine HHDM request.
///
/// The HHDM maps physical memory into a contiguous virtual address range.
#[used]
#[unsafe(link_section = ".limine_reqs")]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

/// Limine memory map request.
#[used]
#[unsafe(link_section = ".limine_reqs")]
pub static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

/// Initialize the kernel memory subsystem.
///
/// This initializes:
///
/// 1. The physical-memory offset supplied by Limine.
/// 2. An `OffsetPageTable` using the currently active page table.
/// 3. The physical frame allocator.
///
/// The heap is initialized separately using [`init_heap`].
///
/// # Safety
///
/// The caller must ensure that:
///
/// - Limine's HHDM response exists.
/// - Limine's memory map response exists.
/// - The current CR3 contains a valid level-4 page table.
/// - The HHDM covers the physical memory that will be accessed.
pub unsafe fn init() -> (
    OffsetPageTable<'static>,
    BootInfoFrameAllocator,
) {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("Limine did not provide an HHDM response");

    let memory_map = MEMMAP_REQUEST
        .response()
        .expect("Limine did not provide a memory-map response");

    let physical_memory_offset =
        VirtAddr::new(hhdm.offset);

    let mapper = unsafe {
        init_mapper(physical_memory_offset)
    };

    let frame_allocator = unsafe {
        BootInfoFrameAllocator::new(
            memory_map.entries(),
            physical_memory_offset,
        )
    };

    (mapper, frame_allocator)
}
pub static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
pub static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

/// Initialize an `OffsetPageTable` using the currently active page table.
///
/// Limine's HHDM gives us a direct mapping from physical memory to virtual
/// memory, which is exactly what `OffsetPageTable` requires.
///
/// # Safety
///
/// The supplied offset must point to a valid HHDM mapping and the currently
/// active CR3 must contain a valid level-4 page table.
pub unsafe fn init_mapper(
    physical_memory_offset: VirtAddr,
) -> OffsetPageTable<'static> {
    let (level_4_frame, _) = Cr3::read();

    let level_4_table_addr =
        physical_memory_offset
            + level_4_frame
                .start_address()
                .as_u64();

    let level_4_table_ptr =
        level_4_table_addr
            .as_mut_ptr::<PageTable>();

    let level_4_table = unsafe {
        &mut *level_4_table_ptr
    };

    unsafe {
        OffsetPageTable::new(
            level_4_table,
            physical_memory_offset,
        )
    }
}

/// Convert a physical address into its HHDM virtual address.
///
/// # Safety
///
/// The supplied physical address must be backed by memory mapped by
/// Limine's HHDM.
pub unsafe fn phys_to_virt(
    physical_address: u64,
) -> VirtAddr {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("Limine did not provide an HHDM response");

    VirtAddr::new(
        hhdm.offset
            .checked_add(physical_address)
            .expect("HHDM address overflow"),
    )
}

/// Convert a physical address into a pointer through the HHDM.
///
/// # Safety
///
/// The physical address must refer to valid memory covered by the HHDM.
///
/// The returned pointer is only valid while the corresponding physical
/// memory remains mapped.
pub unsafe fn phys_to_ptr<T>(
    physical_address: u64,
) -> *mut T {
    unsafe {
        phys_to_virt(physical_address)
            .as_mut_ptr::<T>()
    }
}

/// Print detailed information about the kernel's memory configuration.
///
/// This function is intended for debugging the early memory-management
/// subsystem. It does not allocate or modify memory.
///
/// `frame_allocator` is needed (rather than reading only the Limine
/// statics) so that live allocator state — currently just the free-list
/// frame count — can be reported alongside the static memory-map data.
pub fn mem_analyze(frame_allocator: &BootInfoFrameAllocator) {
    use crate::fb::Color;

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "========================================"
    );

    console_println_color!(
        Color::GREEN,
        "       KERNEL MEMORY ANALYSIS"
    );

    console_println_color!(
        Color::GREEN,
        "========================================"
    );

    // ========================================================
    // HHDM
    // ========================================================

    let hhdm =
        match HHDM_REQUEST.response() {
            Some(response) => response,

            None => {
                console_println_color!(
                    Color::GREEN,
                    "[HHDM] NOT AVAILABLE"
                );

                return;
            }
        };

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[HHDM]"
    );

    console_println_color!(
        Color::GREEN,
        "  Offset:          {:#018x}",
        hhdm.offset
    );

    // ========================================================
    // Memory map
    // ========================================================

    let memory_map =
        match MEMMAP_REQUEST.response() {
            Some(response) => response,

            None => {
                console_println_color!(
                    Color::GREEN,
                    "[MEMORY MAP] NOT AVAILABLE"
                );

                return;
            }
        };

    let entries =
        memory_map.entries();

    let mut total_memory = 0u64;
    let mut usable_memory = 0u64;
    let mut reserved_memory = 0u64;
    let mut reclaimable_memory = 0u64;
    let mut bootloader_memory = 0u64;
    let mut kernel_memory = 0u64;
    let mut framebuffer_memory = 0u64;
    let mut bad_memory = 0u64;
    let mut other_memory = 0u64;

    let mut usable_regions = 0usize;

    let mut first_usable_base = None;
    let mut last_usable_end = None;

    for entry in entries {
        let base = entry.base;
        let length = entry.length;

        let end =
            base.saturating_add(length);

        total_memory =
            total_memory.saturating_add(length);

        match entry.type_ {
            limine::memmap::MEMMAP_USABLE => {
                usable_memory =
                    usable_memory.saturating_add(length);

                usable_regions += 1;

                if first_usable_base.is_none() {
                    first_usable_base =
                        Some(base);
                }

                last_usable_end =
                    Some(
                        last_usable_end
                            .unwrap_or(0)
                            .max(end)
                    );
            }

            limine::memmap::MEMMAP_RESERVED => {
                reserved_memory =
                    reserved_memory.saturating_add(length);
            }

            limine::memmap::MEMMAP_ACPI_RECLAIMABLE => {
                reclaimable_memory =
                    reclaimable_memory.saturating_add(length);
            }

            limine::memmap::MEMMAP_BOOTLOADER_RECLAIMABLE => {
                bootloader_memory =
                    bootloader_memory.saturating_add(length);
            }

            limine::memmap::MEMMAP_EXECUTABLE_AND_MODULES => {
                kernel_memory =
                    kernel_memory.saturating_add(length);
            }

            limine::memmap::MEMMAP_FRAMEBUFFER => {
                framebuffer_memory =
                    framebuffer_memory.saturating_add(length);
            }

            limine::memmap::MEMMAP_BAD_MEMORY => {
                bad_memory =
                    bad_memory.saturating_add(length);
            }

            _ => {
                other_memory =
                    other_memory.saturating_add(length);
            }
        }
    }

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[MEMORY MAP]"
    );

    console_println_color!(
        Color::GREEN,
        "  Entries:         {}",
        entries.len()
    );

    console_println_color!(
        Color::GREEN,
        "  Total:           {} KiB",
        total_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Total:           {} MiB",
        total_memory / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable:          {} MiB",
        usable_memory / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable regions:  {}",
        usable_regions
    );

    console_println_color!(
        Color::GREEN,
        "  Reserved:        {} KiB",
        reserved_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  ACPI reclaim:    {} KiB",
        reclaimable_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Bootloader:      {} KiB",
        bootloader_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Kernel/modules:  {} KiB",
        kernel_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Framebuffer:     {} KiB",
        framebuffer_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Bad memory:      {} KiB",
        bad_memory / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Other:           {} KiB",
        other_memory / 1024
    );

    // ========================================================
    // Physical frames
    // ========================================================

    let frame_count =
        usable_memory / frame::FRAME_SIZE;

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PHYSICAL FRAMES]"
    );

    console_println_color!(
        Color::GREEN,
        "  Frame size:      {} bytes",
        frame::FRAME_SIZE
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:   {}",
        frame_count
    );

    console_println_color!(
        Color::GREEN,
        "  Frame memory:    {} MiB",
        (frame_count * frame::FRAME_SIZE)
            / (1024 * 1024)
    );

    match first_usable_base {
        Some(base) => {
            console_println_color!(
                Color::GREEN,
                "  First usable:    {:#018x}",
                base
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  First usable:    NONE"
            );
        }
    }

    match last_usable_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  Last usable end: {:#018x}",
                end
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  Last usable end: NONE"
            );
        }
    }

    // ========================================================
    // Frame allocator (live state)
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[FRAME ALLOCATOR]"
    );

    let free_list_frames =
        frame_allocator.free_frame_count() as u64;

    console_println_color!(
        Color::GREEN,
        "  Free-list frames: {}",
        free_list_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Free-list memory: {} KiB",
        (free_list_frames * frame::FRAME_SIZE) / 1024
    );

    if frame_count > 0 {
        let free_list_percent =
            (free_list_frames as u128 * 10000)
                / frame_count as u128;

        console_println_color!(
            Color::GREEN,
            "  Free-list share:  {}.{}% of usable frames",
            free_list_percent / 100,
            free_list_percent % 100
        );
    }

    // ========================================================
    // Physical address space
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PHYSICAL ADDRESS SPACE]"
    );

    let mut lowest_address =
        u64::MAX;

    let mut highest_address =
        0u64;

    for entry in entries {
        lowest_address =
            lowest_address.min(
                entry.base
            );

        highest_address =
            highest_address.max(
                entry.base
                    .saturating_add(entry.length)
            );
    }

    if lowest_address != u64::MAX {
        console_println_color!(
            Color::GREEN,
            "  Lowest address:  {:#018x}",
            lowest_address
        );
    }

    console_println_color!(
        Color::GREEN,
        "  Highest address: {:#018x}",
        highest_address
    );

    console_println_color!(
        Color::GREEN,
        "  Address span:    {} MiB",
        highest_address
            .saturating_sub(
                lowest_address
            )
            / (1024 * 1024)
    );

    // ========================================================
    // Memory-map regions
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[MEMORY MAP REGIONS]"
    );

    for (index, entry) in
        entries.iter().enumerate()
    {
        let start =
            entry.base;

        let end =
            entry.base
                .saturating_add(
                    entry.length
                );

        console_println_color!(
            Color::GREEN,
            "  #{}: {:#018x} - {:#018x} | {} KiB | type {}",
            index,
            start,
            end,
            entry.length / 1024,
            entry.type_
        );
    }

    // ========================================================
    // Page tables / CR3
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PAGE TABLES]"
    );

    let (cr3_frame, cr3_flags) =
        Cr3::read();

    let cr3_physical =
        cr3_frame
            .start_address()
            .as_u64();

    console_println_color!(
        Color::GREEN,
        "  CR3 physical:   {:#018x}",
        cr3_physical
    );

    match hhdm.offset.checked_add(
        cr3_physical
    ) {
        Some(address) => {
            console_println_color!(
                Color::GREEN,
                "  CR3 virtual:    {:#018x}",
                address
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  CR3 virtual:    OVERFLOW"
            );
        }
    }

    console_println_color!(
        Color::GREEN,
        "  CR3 flags:      {:?}",
        cr3_flags
    );

    console_println_color!(
        Color::GREEN,
        "  Page size:      4096 bytes"
    );

    // ========================================================
    // HHDM address examples
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[HHDM ADDRESS CONVERSION]"
    );

    if let Some(base) =
        first_usable_base
    {
        match hhdm.offset.checked_add(
            base
        ) {
            Some(virtual_address) => {
                console_println_color!(
                    Color::GREEN,
                    "  Physical:       {:#018x}",
                    base
                );

                console_println_color!(
                    Color::GREEN,
                    "  HHDM virtual:   {:#018x}",
                    virtual_address
                );
            }

            None => {
                console_println_color!(
                    Color::GREEN,
                    "  First usable HHDM address overflow"
                );
            }
        }
    }

    if let Some(end) =
        last_usable_end
    {
        match hhdm.offset.checked_add(
            end
        ) {
            Some(virtual_address) => {
                console_println_color!(
                    Color::GREEN,
                    "  Last physical:  {:#018x}",
                    end
                );

                console_println_color!(
                    Color::GREEN,
                    "  Last HHDM VA:   {:#018x}",
                    virtual_address
                );
            }

            None => {
                console_println_color!(
                    Color::GREEN,
                    "  Last usable HHDM address overflow"
                );
            }
        }
    }

    // ========================================================
    // Kernel heap
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[KERNEL HEAP]"
    );

    let heap_start =
        HEAP_START;

    let heap_size =
        HEAP_SIZE;

    let heap_end =
        heap_start
            .checked_add(
                heap_size
            );

    console_println_color!(
        Color::GREEN,
        "  Start:          {:#018x}",
        heap_start
    );

    match heap_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  End:            {:#018x}",
                end
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  End:            OVERFLOW"
            );
        }
    }

    console_println_color!(
        Color::GREEN,
        "  Size:           {} KiB",
        heap_size / 1024
    );

    console_println_color!(
        Color::GREEN,
        "  Size:           {} MiB",
        heap_size / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Pages:          {}",
        (heap_size + 4095) / 4096
    );

    console_println_color!(
        Color::GREEN,
        "  Start aligned:  {}",
        heap_start % 4096 == 0
    );

    console_println_color!(
        Color::GREEN,
        "  Size aligned:   {}",
        heap_size % 4096 == 0
    );

    match heap_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  End aligned:    {}",
                end % 4096 == 0
            );
        }

        None => {}
    }

    // ========================================================
    // Address-space layout
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[ADDRESS SPACE]"
    );

    console_println_color!(
        Color::GREEN,
        "  HHDM base:      {:#018x}",
        hhdm.offset
    );

    console_println_color!(
        Color::GREEN,
        "  Heap base:      {:#018x}",
        heap_start
    );

    if (heap_start as u64) >= hhdm.offset {
        console_println_color!(
            Color::GREEN,
            "  Heap above HHDM: true"
        );
    } else {
        console_println_color!(
            Color::GREEN,
            "  Heap above HHDM: false"
        );
    }

    // ========================================================
    // Frame capacity
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[FRAME CAPACITY]"
    );

    let heap_frames =
        (heap_size + 4095) / 4096;

    console_println_color!(
        Color::GREEN,
        "  Heap pages:     {}",
        heap_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:  {}",
        frame_count
    );

    if frame_count >= heap_frames as u64 {
        console_println_color!(
            Color::GREEN,
            "  Heap frame cost: {}.{}%",
            (heap_frames as u128 * 10000
                / frame_count as u128)
                / 100,
            (heap_frames as u128 * 10000
                / frame_count as u128)
                % 100
        );
    } else {
        console_println_color!(
            Color::GREEN,
            "  Heap frame cost: exceeds usable-frame count"
        );
    }

    // ========================================================
    // Alignment diagnostics
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[ALIGNMENT]"
    );

    console_println_color!(
        Color::GREEN,
        "  Page size:      {}",
        4096
    );

    console_println_color!(
        Color::GREEN,
        "  HHDM aligned:   {}",
        hhdm.offset % 4096 == 0
    );

    console_println_color!(
        Color::GREEN,
        "  CR3 aligned:    {}",
        cr3_physical % 4096 == 0
    );

    console_println_color!(
        Color::GREEN,
        "  Heap aligned:   {}",
        heap_start % 4096 == 0
    );

    if let Some(base) =
        first_usable_base
    {
        console_println_color!(
            Color::GREEN,
            "  First frame aligned: {}",
            base % 4096 == 0
        );
    }

    // ========================================================
    // Summary
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[SUMMARY]"
    );

    let usable_percent =
        if total_memory != 0 {
            (usable_memory as u128 * 10000)
                / total_memory as u128
        } else {
            0
        };

    console_println_color!(
        Color::GREEN,
        "  Physical RAM:   {} MiB",
        total_memory / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable RAM:     {} MiB",
        usable_memory / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable RAM:     {}.{}%",
        usable_percent / 100,
        usable_percent % 100
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:  {}",
        frame_count
    );

    console_println_color!(
        Color::GREEN,
        "  Free-list:      {} frames",
        free_list_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Heap size:      {} MiB",
        heap_size / (1024 * 1024)
    );

    console_println_color!(
        Color::GREEN,
        "  Heap pages:     {}",
        heap_frames
    );

    console_println_color!(
        Color::GREEN,
        "  HHDM:           {:#018x}",
        hhdm.offset
    );

    console_println_color!(
        Color::GREEN,
        "  CR3:            {:#018x}",
        cr3_physical
    );

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "========================================"
    );

    console_println_color!(
        Color::GREEN,
        "       END MEMORY ANALYSIS"
    );

    console_println_color!(
        Color::GREEN,
        "========================================"
    );

    console_println_color!(
        Color::GREEN,
        ""
    );
}
