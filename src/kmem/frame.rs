//! Physical frame allocation.
//!
//! This allocator uses Limine's memory map as the source of usable
//! physical memory. Frames are handed out from a small free list first
//! (populated by `deallocate_frame`), and otherwise sequentially from
//! the memory map, skipping any range marked reserved via [`Self::reserve`].
//!
//! The free list is intrusive: each freed frame stores a pointer to the
//! next free frame *inside itself*, so there is no bound on how many
//! frames can be tracked. This requires being able to read/write physical
//! memory through a linear mapping (e.g. Limine's HHDM), so the allocator
//! must be constructed with that offset.
//!
//! This is suitable for the early stages of the kernel. It can later be
//! replaced by a bitmap, buddy allocator, or another physical allocator.

use limine::memmap::{
    Entry,
    MEMMAP_USABLE,
};

use x86_64::{
    structures::paging::{
        FrameAllocator,
        FrameDeallocator,
        PhysFrame,
        Size4KiB,
    },
    PhysAddr,
    VirtAddr,
};

#[derive(Clone, Copy)]
pub struct PhysRange {
    pub start: u64,
    pub end: u64,
}

/// Size of an x86_64 4 KiB frame.
pub const FRAME_SIZE: u64 = 4096;

const MAX_RESERVED_RANGES: usize = 32;

/// Sentinel used in the intrusive free list to mean "no next frame".
/// A real physical address will never equal this on any supported platform.
const FREE_LIST_END: u64 = u64::MAX;

/// Physical frame allocator backed by Limine's memory map.
pub struct BootInfoFrameAllocator {
    memory_map: &'static [&'static Entry],

    /// Offset added to a physical address to obtain a virtual address
    /// through which that physical memory can be accessed directly
    /// (e.g. Limine's higher-half direct map). Required so that freed
    /// frames can store their free-list link inside themselves.
    phys_mem_offset: VirtAddr,

    /// Index of the memory-map region currently being allocated from.
    current_region: usize,

    /// Physical address of the next frame in the current region.
    next_frame: u64,

    /// Reserved physical memory.
    reserved: [Option<PhysRange>; MAX_RESERVED_RANGES],
    reserved_count: usize,

    /// Head of the intrusive free list of previously deallocated frames.
    /// `None` means the list is empty.
    free_list_head: Option<u64>,
}

impl BootInfoFrameAllocator {
    /// Create a frame allocator from Limine's memory map.
    ///
    /// `phys_mem_offset` must be the offset of a mapping that covers all
    /// physical memory (e.g. Limine's HHDM response), since it is used
    /// to read and write freed frames in place.
    ///
    /// # Safety
    ///
    /// The memory map must remain valid for the lifetime of this allocator.
    ///
    /// The caller must ensure that all memory reported as usable by Limine
    /// is actually available for physical frame allocation, except for
    /// ranges explicitly reserved with [`Self::reserve`], and that
    /// `phys_mem_offset` really does map all physical memory read/write.
    pub unsafe fn new(
        memory_map: &'static [&'static Entry],
        phys_mem_offset: VirtAddr,
    ) -> Self {
        Self {
            memory_map,
            phys_mem_offset,
            current_region: 0,
            next_frame: 0,
            reserved: [None; MAX_RESERVED_RANGES],
            reserved_count: 0,
            free_list_head: None,
        }
    }

    /// Return the total amount of usable physical memory reported by Limine.
    ///
    /// This value does not account for memory explicitly reserved by the
    /// allocator.
    pub fn usable_memory(&self) -> u64 {
        self.memory_map
            .iter()
            .filter(|region| region.type_ == MEMMAP_USABLE)
            .map(|region| region.length)
            .sum()
    }

    /// Reserve a physical memory range so that it will never be allocated.
    ///
    /// `start` is the physical start address and `size` is the size in bytes.
    ///
    /// The range does not need to be page-aligned; frame allocation will
    /// automatically avoid any 4 KiB frames that overlap it.
    ///
    /// Must be called before any frames are allocated or freed, since
    /// ranges reserved afterwards are not retroactively removed from the
    /// free list.
    pub fn reserve(&mut self, start: u64, size: u64) {
        let end = start
            .checked_add(size)
            .expect("physical address range overflow");

        assert!(
            self.reserved_count < MAX_RESERVED_RANGES,
            "too many reserved physical ranges"
        );

        self.reserved[self.reserved_count] = Some(PhysRange { start, end });
        self.reserved_count += 1;
    }

    /// Count how many frames are currently sitting in the free list.
    ///
    /// This walks the whole list, so it's O(n) in the number of freed
    /// frames — intended for occasional diagnostics (e.g. `mem_analyze`),
    /// not a hot path.
    pub fn free_frame_count(&self) -> usize {
        let mut count = 0;
        let mut current = self.free_list_head;

        while let Some(frame) = current {
            count += 1;

            let node_ptr = self.phys_to_virt(frame).as_ptr::<u64>();
            let next = unsafe { node_ptr.read() };

            current = if next == FREE_LIST_END {
                None
            } else {
                Some(next)
            };
        }

        count
    }

    /// Check whether a physical frame lies inside a reserved range.
    fn is_reserved(&self, frame: u64) -> bool {
        self.reserved[..self.reserved_count]
            .iter()
            .flatten()
            .any(|range| frame >= range.start && frame < range.end)
    }

    /// Advance `frame` past any reserved range(s) it currently overlaps.
    ///
    /// Reserved ranges are not required to be disjoint or sorted, so this
    /// repeats until `frame` no longer overlaps any of them.
    fn skip_reserved(&self, mut frame: u64) -> u64 {
        loop {
            let mut advanced = false;

            for range in self.reserved[..self.reserved_count].iter().flatten() {
                if frame >= range.start && frame < range.end {
                    let past_end = align_up(range.end, FRAME_SIZE);

                    if past_end > frame {
                        frame = past_end;
                        advanced = true;
                    }
                }
            }

            if !advanced {
                return frame;
            }
        }
    }

    /// Find the next usable memory region.
    fn next_usable_region(&mut self) -> Option<&'static Entry> {
        while self.current_region < self.memory_map.len() {
            let region = self.memory_map[self.current_region];

            if region.type_ == MEMMAP_USABLE {
                return Some(region);
            }

            self.current_region += 1;
            self.next_frame = 0;
        }

        None
    }

    /// Physical address -> virtual address through the direct map.
    fn phys_to_virt(&self, phys: u64) -> VirtAddr {
        self.phys_mem_offset + phys
    }

    /// Push a frame onto the intrusive free list.
    ///
    /// # Safety
    ///
    /// `frame` must be a valid, currently-unused physical frame that is
    /// mapped read/write through `phys_mem_offset`.
    unsafe fn push_free_frame(&mut self, frame: u64) {
        let next = self.free_list_head.unwrap_or(FREE_LIST_END);
        let node_ptr = self.phys_to_virt(frame).as_mut_ptr::<u64>();

        unsafe {
            node_ptr.write(next);
        }

        self.free_list_head = Some(frame);
    }

    /// Pop a frame from the intrusive free list, if any are available.
    fn pop_free_frame(&mut self) -> Option<u64> {
        let frame = self.free_list_head?;
        let node_ptr = self.phys_to_virt(frame).as_ptr::<u64>();

        let next = unsafe { node_ptr.read() };

        self.free_list_head = if next == FREE_LIST_END {
            None
        } else {
            Some(next)
        };

        Some(frame)
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        // Prefer previously freed frames over untouched memory-map space.
        if let Some(frame) = self.pop_free_frame() {
            return Some(PhysFrame::containing_address(PhysAddr::new(frame)));
        }

        loop {
            let region = self.next_usable_region()?;

            let region_start = align_up(region.base, FRAME_SIZE);
            let region_end =
                align_down(region.base.checked_add(region.length)?, FRAME_SIZE);

            // Start at the beginning of this region.
            if self.next_frame == 0 {
                self.next_frame = region_start;
            }

            // Skip past any reserved ranges before checking bounds.
            self.next_frame = self.skip_reserved(self.next_frame);

            // Check whether the region still has frames.
            if self.next_frame < region_end {
                let frame =
                    PhysFrame::containing_address(PhysAddr::new(self.next_frame));

                self.next_frame += FRAME_SIZE;

                return Some(frame);
            }

            // Move to the next memory-map region.
            self.current_region += 1;
            self.next_frame = 0;
        }
    }
}

impl FrameDeallocator<Size4KiB> for BootInfoFrameAllocator {
    /// Return a frame to the allocator so it can be reused.
    ///
    /// # Safety
    ///
    /// `frame` must not still be in use anywhere (no live mappings,
    /// no other owner), and must have originally come from this
    /// allocator (or otherwise be safe to hand out again).
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        unsafe {
            self.push_free_frame(frame.start_address().as_u64());
        }
    }
}

/// Align an address upwards.
#[inline]
pub const fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

/// Align an address downwards.
#[inline]
pub const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}
