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
pub use heap::{HEAP_SIZE, HEAP_START};

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
/// subsystem. It does not allocate memory or modify memory.
///
/// The report deliberately distinguishes between:
///
/// - physical address-space coverage,
/// - usable RAM,
/// - bootloader/kernel/reclaimable memory,
/// - reserved/MMIO regions,
/// - physical frames,
/// - page tables,
/// - and the kernel heap.
///
/// This is important because the Limine memory map describes the physical
/// address space, not simply the amount of installed RAM.
pub fn mem_analyze(frame_allocator: &BootInfoFrameAllocator) {
    use crate::fb::Color;

    const PAGE_SIZE: usize = frame::FRAME_SIZE as usize;

    fn kib(bytes: u64) -> u64 {
        bytes / 1024
    }

    fn mib(bytes: u64) -> u64 {
        bytes / (1024 * 1024)
    }

    fn percent_x100(part: u64, total: u64) -> u64 {
        if total == 0 {
            0
        } else {
            ((part as u128 * 10_000) / total as u128) as u64
        }
    }

    fn print_percent(
        label: &str,
        part: u64,
        total: u64,
    ) {
        let p = percent_x100(part, total);

        console_println_color!(
            Color::GREEN,
            "  {:<20} {}.{}%",
            label,
            p / 100,
            p % 100
        );
    }

    fn memory_type_name(type_: u64) -> &'static str {
        match type_ {
            limine::memmap::MEMMAP_USABLE => "USABLE",
            limine::memmap::MEMMAP_RESERVED => "RESERVED",
            limine::memmap::MEMMAP_ACPI_RECLAIMABLE => "ACPI RECLAIMABLE",
            limine::memmap::MEMMAP_ACPI_NVS => "ACPI NVS",
            limine::memmap::MEMMAP_BAD_MEMORY => "BAD MEMORY",
            limine::memmap::MEMMAP_BOOTLOADER_RECLAIMABLE => {
                "BOOTLOADER RECLAIMABLE"
            }
            limine::memmap::MEMMAP_EXECUTABLE_AND_MODULES => {
                "EXECUTABLE / MODULES"
            }
            limine::memmap::MEMMAP_FRAMEBUFFER => "FRAMEBUFFER",
            _ => "UNKNOWN",
        }
    }

    fn contains(
        base: u64,
        length: u64,
        address: u64,
    ) -> bool {
        let Some(end) =
            base.checked_add(length)
        else {
            return false;
        };

        address >= base && address < end
    }

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
    // Requests / responses
    // ========================================================

    let hhdm =
        match HHDM_REQUEST.response() {
            Some(response) => response,

            None => {
                console_println_color!(
                    Color::GREEN,
                    ""
                );

                console_println_color!(
                    Color::GREEN,
                    "[ERROR]"
                );

                console_println_color!(
                    Color::GREEN,
                    "  Limine HHDM response unavailable"
                );

                return;
            }
        };

    let memory_map =
        match MEMMAP_REQUEST.response() {
            Some(response) => response,

            None => {
                console_println_color!(
                    Color::GREEN,
                    ""
                );

                console_println_color!(
                    Color::GREEN,
                    "[ERROR]"
                );

                console_println_color!(
                    Color::GREEN,
                    "  Limine memory-map response unavailable"
                );

                return;
            }
        };

    let entries =
        memory_map.entries();

    // ========================================================
    // HHDM
    // ========================================================

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
        "  Offset:             {:#018x}",
        hhdm.offset
    );

    console_println_color!(
        Color::GREEN,
        "  Page aligned:       {}",
        hhdm.offset % PAGE_SIZE as u64 == 0
    );

    // ========================================================
    // Memory-map accounting
    // ========================================================

    let mut address_map_bytes = 0u64;

    let mut usable_bytes = 0u64;
    let mut reserved_bytes = 0u64;
    let mut acpi_reclaim_bytes = 0u64;
    let mut acpi_nvs_bytes = 0u64;
    let mut bootloader_bytes = 0u64;
    let mut kernel_bytes = 0u64;
    let mut framebuffer_bytes = 0u64;
    let mut bad_bytes = 0u64;
    let mut other_bytes = 0u64;

    let mut usable_regions = 0usize;

    let mut first_usable_base: Option<u64> = None;
    let mut last_usable_end: Option<u64> = None;

    let mut largest_usable_base = 0u64;
    let mut largest_usable_length = 0u64;

    for entry in entries {
        let base = entry.base;
        let length = entry.length;

        let end =
            match base.checked_add(length) {
                Some(end) => end,

                None => {
                    continue;
                }
            };

        address_map_bytes =
            address_map_bytes.saturating_add(length);

        match entry.type_ {
            limine::memmap::MEMMAP_USABLE => {
                usable_bytes =
                    usable_bytes.saturating_add(length);

                usable_regions += 1;

                first_usable_base =
                    Some(
                        first_usable_base
                            .map_or(base, |old| old.min(base))
                    );

                last_usable_end =
                    Some(
                        last_usable_end
                            .map_or(end, |old| old.max(end))
                    );

                if length > largest_usable_length {
                    largest_usable_length = length;
                    largest_usable_base = base;
                }
            }

            limine::memmap::MEMMAP_RESERVED => {
                reserved_bytes =
                    reserved_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_ACPI_RECLAIMABLE => {
                acpi_reclaim_bytes =
                    acpi_reclaim_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_ACPI_NVS => {
                acpi_nvs_bytes =
                    acpi_nvs_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_BOOTLOADER_RECLAIMABLE => {
                bootloader_bytes =
                    bootloader_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_EXECUTABLE_AND_MODULES => {
                kernel_bytes =
                    kernel_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_FRAMEBUFFER => {
                framebuffer_bytes =
                    framebuffer_bytes.saturating_add(length);
            }

            limine::memmap::MEMMAP_BAD_MEMORY => {
                bad_bytes =
                    bad_bytes.saturating_add(length);
            }

            _ => {
                other_bytes =
                    other_bytes.saturating_add(length);
            }
        }
    }

    // ========================================================
    // Physical memory
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PHYSICAL MEMORY]"
    );

    console_println_color!(
        Color::GREEN,
        "  Address-map bytes:  {}",
        address_map_bytes
    );

    console_println_color!(
        Color::GREEN,
        "  Address-map span:   {} MiB",
        mib(address_map_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable RAM:         {} MiB",
        mib(usable_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable regions:     {}",
        usable_regions
    );

    console_println_color!(
        Color::GREEN,
        "  Largest usable:     {} MiB",
        mib(largest_usable_length)
    );

    if largest_usable_length != 0 {
        console_println_color!(
            Color::GREEN,
            "  Largest base:       {:#018x}",
            largest_usable_base
        );
    }

    console_println_color!(
        Color::GREEN,
        ""
    );

    print_percent(
        "Usable",
        usable_bytes,
        address_map_bytes,
    );

    print_percent(
        "Reserved",
        reserved_bytes,
        address_map_bytes,
    );

    print_percent(
        "Bootloader",
        bootloader_bytes,
        address_map_bytes,
    );

    print_percent(
        "Kernel/modules",
        kernel_bytes,
        address_map_bytes,
    );

    print_percent(
        "Framebuffer",
        framebuffer_bytes,
        address_map_bytes,
    );

    // ========================================================
    // Memory-map types
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[MEMORY MAP TYPES]"
    );

    console_println_color!(
        Color::GREEN,
        "  RESERVED:           {} KiB",
        kib(reserved_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  ACPI reclaimable:   {} KiB",
        kib(acpi_reclaim_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  ACPI NVS:           {} KiB",
        kib(acpi_nvs_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Bootloader reclaim: {} KiB",
        kib(bootloader_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Kernel/modules:     {} KiB",
        kib(kernel_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Framebuffer:        {} KiB",
        kib(framebuffer_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Bad memory:         {} KiB",
        kib(bad_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Other:              {} KiB",
        kib(other_bytes)
    );

    // ========================================================
    // Physical frames
    // ========================================================

    let usable_frames =
        entries
            .iter()
            .filter(|entry| {
                entry.type_
                    == limine::memmap::MEMMAP_USABLE
            })
            .map(|entry| {
                entry.length / PAGE_SIZE as u64
            })
            .sum::<u64>();

    let usable_frame_bytes =
        usable_frames
            .saturating_mul(PAGE_SIZE as u64);

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
        "  Frame size:         {} bytes",
        PAGE_SIZE
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:      {}",
        usable_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frame RAM:   {} MiB",
        mib(usable_frame_bytes)
    );

    match first_usable_base {
        Some(base) => {
            console_println_color!(
                Color::GREEN,
                "  First usable:       {:#018x}",
                base
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  First usable:       NONE"
            );
        }
    }

    match last_usable_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  Last usable end:    {:#018x}",
                end
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  Last usable end:    NONE"
            );
        }
    }

    // ========================================================
    // Frame allocator
    // ========================================================

    let free_frames =
        frame_allocator
            .free_frame_count() as u64;

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[FRAME ALLOCATOR]"
    );

    console_println_color!(
        Color::GREEN,
        "  Total usable:       {} frames",
        usable_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Reported free:      {} frames",
        free_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Reported free RAM:  {} MiB",
        mib(
            free_frames
                .saturating_mul(PAGE_SIZE as u64)
        )
    );

    if usable_frames > 0 {
        print_percent(
            "Free frame share",
            free_frames,
            usable_frames,
        );
    }

    // ========================================================
    // Physical address range
    // ========================================================

    let mut lowest_address =
        u64::MAX;

    let mut highest_address =
        0u64;

    for entry in entries {
        lowest_address =
            lowest_address.min(entry.base);

        if let Some(end) =
            entry.base.checked_add(entry.length)
        {
            highest_address =
                highest_address.max(end);
        }
    }

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PHYSICAL ADDRESS SPACE]"
    );

    if lowest_address != u64::MAX {
        console_println_color!(
            Color::GREEN,
            "  Lowest mapped:      {:#018x}",
            lowest_address
        );
    } else {
        console_println_color!(
            Color::GREEN,
            "  Lowest mapped:      NONE"
        );
    }

    console_println_color!(
        Color::GREEN,
        "  Highest boundary:   {:#018x}",
        highest_address
    );

    console_println_color!(
        Color::GREEN,
        "  Boundary span:      {} MiB",
        mib(
            highest_address
                .saturating_sub(lowest_address)
        )
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
            start
                .checked_add(entry.length)
                .unwrap_or(u64::MAX);

        console_println_color!(
            Color::GREEN,
            "  #{:<2} {:#018x} - {:#018x} | {:>8} KiB | {}",
            index,
            start,
            end,
            kib(entry.length),
            memory_type_name(entry.type_)
        );
    }

    // ========================================================
    // Page tables / CR3
    // ========================================================

    let (cr3_frame, cr3_flags) =
        Cr3::read();

    let cr3_physical =
        cr3_frame
            .start_address()
            .as_u64();

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[PAGE TABLES]"
    );

    console_println_color!(
        Color::GREEN,
        "  CR3 physical:      {:#018x}",
        cr3_physical
    );

    match hhdm.offset.checked_add(cr3_physical) {
        Some(cr3_virtual) => {
            console_println_color!(
                Color::GREEN,
                "  CR3 virtual:       {:#018x}",
                cr3_virtual
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  CR3 virtual:       OVERFLOW"
            );
        }
    }

    console_println_color!(
        Color::GREEN,
        "  CR3 flags:         {:?}",
        cr3_flags
    );

    console_println_color!(
        Color::GREEN,
        "  Page size:         {} bytes",
        PAGE_SIZE
    );

    console_println_color!(
        Color::GREEN,
        "  CR3 aligned:       {}",
        cr3_physical % PAGE_SIZE as u64 == 0
    );

    // Find which memory-map region contains CR3.
    let mut cr3_region_type: Option<u32> =
        None;

    for entry in entries {
        if contains(
            entry.base,
            entry.length,
            cr3_physical,
        ) {
            cr3_region_type =
                Some(entry.type_ as u32);

            break;
        }
    }

    match cr3_region_type {
        Some(type_) => {
            console_println_color!(
                Color::GREEN,
                "  CR3 region:        {}",
                memory_type_name(type_ as u64)
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  CR3 region:        NOT FOUND"
            );
        }
    }

    // ========================================================
    // HHDM conversion
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
        match hhdm.offset.checked_add(base) {
            Some(virtual_address) => {
                console_println_color!(
                    Color::GREEN,
                    "  First physical:    {:#018x}",
                    base
                );

                console_println_color!(
                    Color::GREEN,
                    "  First HHDM VA:     {:#018x}",
                    virtual_address
                );
            }

            None => {
                console_println_color!(
                    Color::GREEN,
                    "  First HHDM VA:     OVERFLOW"
                );
            }
        }
    }

    if let Some(end) =
        last_usable_end
    {
        match hhdm.offset.checked_add(end) {
            Some(virtual_address) => {
                console_println_color!(
                    Color::GREEN,
                    "  Last physical:     {:#018x}",
                    end
                );

                console_println_color!(
                    Color::GREEN,
                    "  Last HHDM VA:      {:#018x}",
                    virtual_address
                );
            }

            None => {
                console_println_color!(
                    Color::GREEN,
                    "  Last HHDM VA:      OVERFLOW"
                );
            }
        }
    }

    // ========================================================
    // Kernel heap
    // ========================================================

    let heap_start =
        HEAP_START;

    let heap_size =
        HEAP_SIZE;

    let heap_end =
        heap_start.checked_add(heap_size);

    let heap_pages =
        (heap_size + PAGE_SIZE - 1)
            / PAGE_SIZE;

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[KERNEL HEAP]"
    );

    console_println_color!(
        Color::GREEN,
        "  Start:             {:#018x}",
        heap_start
    );

    match heap_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  End:               {:#018x}",
                end
            );
        }

        None => {
            console_println_color!(
                Color::GREEN,
                "  End:               OVERFLOW"
            );
        }
    }

    console_println_color!(
        Color::GREEN,
        "  Size:              {} KiB",
        kib(heap_size as u64)
    );

    console_println_color!(
        Color::GREEN,
        "  Size:              {} MiB",
        mib(heap_size as u64)
    );

    console_println_color!(
        Color::GREEN,
        "  Pages:             {}",
        heap_pages
    );

    console_println_color!(
        Color::GREEN,
        "  Start aligned:     {}",
        heap_start % PAGE_SIZE == 0
    );

    console_println_color!(
        Color::GREEN,
        "  Size aligned:      {}",
        heap_size % PAGE_SIZE == 0
    );

    match heap_end {
        Some(end) => {
            console_println_color!(
                Color::GREEN,
                "  End aligned:       {}",
                end % PAGE_SIZE == 0
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
        "  HHDM base:         {:#018x}",
        hhdm.offset
    );

    console_println_color!(
        Color::GREEN,
        "  Heap base:         {:#018x}",
        heap_start
    );

    console_println_color!(
        Color::GREEN,
        "  Heap < HHDM:       {}",
        heap_start < hhdm.offset as usize
    );

    if let Some(end) = heap_end {
        console_println_color!(
            Color::GREEN,
            "  Heap end:          {:#018x}",
            end
        );

        console_println_color!(
            Color::GREEN,
            "  Heap overlaps HHDM: {}",
            heap_start < hhdm.offset as usize
                && end > hhdm.offset as usize
        );
    }

    // ========================================================
    // Heap / frame capacity
    // ========================================================

    console_println_color!(
        Color::GREEN,
        ""
    );

    console_println_color!(
        Color::GREEN,
        "[FRAME CAPACITY]"
    );

    console_println_color!(
        Color::GREEN,
        "  Heap pages:        {}",
        heap_pages
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:     {}",
        usable_frames
    );

    if usable_frames > 0 {
        print_percent(
            "Heap / usable",
            heap_pages as u64,
            usable_frames,
        );
    }

    if heap_pages > usable_frames as usize {
        console_println_color!(
            Color::GREEN,
            "  WARNING:           Heap exceeds usable frames"
        );
    }

    // ========================================================
    // Alignment
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
        "  Page size:         {}",
        PAGE_SIZE
    );

    console_println_color!(
        Color::GREEN,
        "  HHDM aligned:      {}",
        hhdm.offset % PAGE_SIZE as u64 == 0
    );

    console_println_color!(
        Color::GREEN,
        "  CR3 aligned:       {}",
        cr3_physical % PAGE_SIZE as u64 == 0
    );

    console_println_color!(
        Color::GREEN,
        "  Heap aligned:      {}",
        heap_start % PAGE_SIZE == 0
    );

    if let Some(base) =
        first_usable_base
    {
        console_println_color!(
            Color::GREEN,
            "  First frame:       {}",
            base % PAGE_SIZE as u64 == 0
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

    console_println_color!(
        Color::GREEN,
        "  RAM usable:        {} MiB",
        mib(usable_frame_bytes)
    );

    console_println_color!(
        Color::GREEN,
        "  Usable frames:     {}",
        usable_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Free frames:       {}",
        free_frames
    );

    console_println_color!(
        Color::GREEN,
        "  Heap:              {} MiB",
        mib(heap_size as u64)
    );

    console_println_color!(
        Color::GREEN,
        "  Heap pages:        {}",
        heap_pages
    );

    console_println_color!(
        Color::GREEN,
        "  HHDM:              {:#018x}",
        hhdm.offset
    );

    console_println_color!(
        Color::GREEN,
        "  CR3:               {:#018x}",
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
