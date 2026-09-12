//! A binary buddy allocator.
//!
//! Free memory is tracked as power-of-two sized blocks. Each order (block
//! size) has its own intrusive free list, stored inside the free blocks
//! themselves — no separate metadata heap is required.
//!
//! Allocating a block that isn't available at the requested order splits
//! the next larger free block in half, pushing the unused half ("buddy")
//! onto a lower free list. Freeing a block checks whether its buddy is
//! also free and, if so, merges the two into the next order up, repeating
//! until no further merge is possible.
//!
//! ## Zones
//!
//! A heap size like Oxenna's 100 MiB is not itself a power of two, so
//! `init` decomposes it into the largest aligned power-of-two chunks
//! that fit (e.g. 64 MiB + 32 MiB + 4 MiB). Each chunk is tracked as an
//! independent "zone" with its own base address and top order.
//!
//! This matters because the classic buddy address formula,
//! `buddy = addr XOR block_size(order)`, is only valid when computed
//! relative to a base that is itself aligned to a power of two at least
//! as large as the block being merged. Computing it relative to one
//! global `heap_start` across multiple differently-sized chunks can
//! produce an address that isn't the block's real buddy at all — and if
//! that bogus address happens to coincide with an unrelated free block
//! of the same order in a different chunk, two non-adjacent blocks would
//! be merged into one, corrupting the heap.
//!
//! Tracking zones separately fixes this: a merge is only ever attempted
//! relative to the base of the zone the block actually lives in, and is
//! only allowed up to that zone's own top order, so it can never reach
//! outside the zone's bounds.

use core::{
    alloc::Layout,
    cmp,
    mem::size_of,
    ptr,
};

use super::MemoryAllocator;

/// Smallest block order the allocator will hand out (2^MIN_ORDER bytes).
const MIN_ORDER: usize = 6; // 64 bytes

/// Largest block order the allocator supports (2^MAX_ORDER bytes).
const MAX_ORDER: usize = 30; // 1 GiB

/// Number of distinct block sizes tracked by the allocator.
const ORDER_COUNT: usize = MAX_ORDER - MIN_ORDER + 1;

/// Maximum number of power-of-two chunks a non-power-of-two heap can be
/// decomposed into. A heap size that is itself a power of two needs
/// just one; pathological sizes need at most one per bit.
const MAX_ZONES: usize = 32;

/// Intrusive free-list node stored inside a free block.
struct FreeListNode {
    next: *mut FreeListNode,
}

/// A single power-of-two-aligned chunk of the heap.
///
/// All buddy merging for a block is bounded to the zone containing it:
/// buddy addresses are computed relative to `base`, and merging stops at
/// `order` (the zone's own top order), so it can never wander into a
/// neighboring zone.
#[derive(Clone, Copy)]
pub struct Zone {
    pub base: usize,
    pub order: usize,
}

/// A binary buddy allocator.
///
/// Free memory is stored as `ORDER_COUNT` singly-linked lists, one per
/// block size (`2^MIN_ORDER ..= 2^max_order` bytes). The lists themselves
/// live inside the free blocks:
///
/// ```text
/// free_lists[order] -> [ FreeListNode | free memory ] -> [ FreeListNode | free memory ] -> null
/// ```
pub struct BuddyAllocator {
    /// Largest order used by any zone; the ceiling for `allocate_order`.
    pub max_order: usize,
    pub free_lists: [*mut FreeListNode; ORDER_COUNT],
    pub zones: [Option<Zone>; MAX_ZONES],
    pub zone_count: usize,
    pub initialized: bool,
}

// `free_lists` and `zones` hold raw pointers/addresses into heap memory
// owned exclusively by this allocator, so it is safe to move/share across
// threads under the same synchronization the caller already applies
// (e.g. a `Mutex`).
unsafe impl Send for BuddyAllocator {}

impl BuddyAllocator {
    /// Create an uninitialized buddy allocator.
    pub const fn new() -> Self {
        Self {
            max_order: MIN_ORDER,
            free_lists: [ptr::null_mut(); ORDER_COUNT],
            zones: [None; MAX_ZONES],
            zone_count: 0,
            initialized: false,
        }
    }

    /// Initialize the allocator over a contiguous memory range.
    ///
    /// The range does not need to be a power-of-two size: it is greedily
    /// decomposed into the largest aligned power-of-two chunks that fit,
    /// each tracked as its own zone, so a `heap_size` like Oxenna's
    /// 100 MiB heap works without waste beyond the final, sub-minimum-
    /// block remainder (if any).
    ///
    /// # Safety
    ///
    /// `heap_start..heap_start + heap_size` must be valid writable memory
    /// that is exclusively owned by this allocator, and `heap_start` must
    /// be aligned to at least `2^MIN_ORDER`.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        assert!(
            heap_start % Self::block_size(MIN_ORDER) == 0,
            "heap start is not aligned to the minimum block size"
        );

        assert!(
            heap_size >= Self::block_size(MIN_ORDER),
            "heap is too small"
        );

        self.free_lists = [ptr::null_mut(); ORDER_COUNT];
        self.zones = [None; MAX_ZONES];
        self.zone_count = 0;
        self.max_order = MIN_ORDER;

        let heap_max_order = Self::largest_order_for(heap_size).min(MAX_ORDER);

        let mut addr = heap_start;
        let mut remaining = heap_size;

        // Decompose the heap into the largest aligned power-of-two chunks
        // that fit, largest first. Each chunk becomes its own zone and
        // its own initial free block.
        while remaining >= Self::block_size(MIN_ORDER) {
            let mut order = heap_max_order.min(Self::largest_order_for(remaining));

            while order > MIN_ORDER
                && (Self::block_size(order) > remaining || addr % Self::block_size(order) != 0)
            {
                order -= 1;
            }

            let size = Self::block_size(order);

            if size > remaining || addr % size != 0 {
                break;
            }

            assert!(self.zone_count < MAX_ZONES, "heap decomposes into too many zones");

            self.zones[self.zone_count] = Some(Zone { base: addr, order });
            self.zone_count += 1;
            self.max_order = self.max_order.max(order);

            unsafe {
                self.push_free_block(addr, order);
            }

            addr += size;
            remaining -= size;
        }

        self.initialized = true;
    }

    /// Block size in bytes for a given order.
    #[inline]
    const fn block_size(order: usize) -> usize {
        1 << order
    }

    /// Index into `free_lists` for a given order.
    #[inline]
    const fn index_for(order: usize) -> usize {
        order - MIN_ORDER
    }

    /// Largest order whose block size is `<= size`.
    fn largest_order_for(size: usize) -> usize {
        let mut order = MIN_ORDER;

        while order < MAX_ORDER && Self::block_size(order + 1) <= size {
            order += 1;
        }

        order
    }

    /// Smallest order able to satisfy an allocation of `size` bytes.
    fn order_for_size(size: usize) -> Option<usize> {
        let size = cmp::max(size, size_of::<FreeListNode>());
        let mut order = MIN_ORDER;

        while Self::block_size(order) < size {
            if order == MAX_ORDER {
                return None;
            }

            order += 1;
        }

        Some(order)
    }

    /// Find the zone containing `addr`, if any.
    pub fn zone_for(&self, addr: usize) -> Option<Zone> {
        self.zones[..self.zone_count]
            .iter()
            .flatten()
            .copied()
            .find(|zone| {
                let size = Self::block_size(zone.order);
                addr >= zone.base && addr < zone.base + size
            })
    }

    /// Compute the address of a block's buddy at the given order, relative
    /// to the base of the zone it lives in.
    #[inline]
    fn buddy_addr(zone: Zone, addr: usize, order: usize) -> Option<usize> {
        let block_size = Self::block_size(order);
        let zone_size = Self::block_size(zone.order);

        let offset = addr.checked_sub(zone.base)?;

        // The block must be inside the zone.
        if offset >= zone_size {
            return None;
        }

        // The block must be aligned to this order relative to the zone.
        if offset % block_size != 0 {
            return None;
        }

        let buddy_offset = offset ^ block_size;

        // Buddy must remain inside this zone.
        if buddy_offset >= zone_size {
            return None;
        }

        Some(zone.base + buddy_offset)
    }

    /// Push a free block of the given order onto its free list.
    ///
    /// # Safety
    ///
    /// `addr` must point to a valid, unused, and properly aligned region
    /// of at least `block_size(order)` bytes.
    unsafe fn push_free_block(&mut self, addr: usize, order: usize) {
        let node = addr as *mut FreeListNode;
        let index = Self::index_for(order);

        unsafe {
            (*node).next = self.free_lists[index];
        }

        self.free_lists[index] = node;
    }

    /// Pop a free block of the given order from its free list, if any.
    fn pop_free_block(&mut self, order: usize) -> Option<usize> {
        let index = Self::index_for(order);
        let node = self.free_lists[index];

        if node.is_null() {
            return None;
        }

        unsafe {
            self.free_lists[index] = (*node).next;
        }

        Some(node as usize)
    }

    /// Remove a specific block from its free list, returning whether it
    /// was found.
    fn remove_free_block(&mut self, addr: usize, order: usize) -> bool {
        let index = Self::index_for(order);
        let target = addr as *mut FreeListNode;

        let mut prev: *mut FreeListNode = ptr::null_mut();
        let mut current = self.free_lists[index];

        while !current.is_null() {
            if current == target {
                let next = unsafe { (*current).next };

                if prev.is_null() {
                    self.free_lists[index] = next;
                } else {
                    unsafe {
                        (*prev).next = next;
                    }
                }

                return true;
            }

            prev = current;
            current = unsafe { (*current).next };
        }

        false
    }

    pub fn contains_free_block(
        &self,
        addr: usize,
        order: usize,
    ) -> bool {
        let index = Self::index_for(order);
        let target = addr as *mut FreeListNode;

        let mut current = self.free_lists[index];

        while !current.is_null() {
            if current == target {
                return true;
            }

            current = unsafe {
                (*current).next
            };
        }

        false
    }

    /// Allocate a block of the given order, splitting a larger free block
    /// if none of the exact size is available.
    ///
    /// Splitting never needs zone information: a block being split is by
    /// construction wholly contained within a single zone (zones are
    /// only ever subdivided, never merged into each other), so cutting it
    /// in half at `addr` and `addr + block_size(order)` is always safe.
    fn allocate_order(&mut self, order: usize) -> Option<usize> {
        if order > self.max_order {
            return None;
        }

        if let Some(addr) = self.pop_free_block(order) {
            return Some(addr);
        }

        // No free block of this order; split the next larger one and
        // keep the unused half for future allocations.
        let addr = self.allocate_order(order + 1)?;
        let buddy_addr = addr + Self::block_size(order);

        unsafe {
            self.push_free_block(buddy_addr, order);
        }

        Some(addr)
    }
}

impl MemoryAllocator for BuddyAllocator {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return ptr::null_mut();
        }

        let size = cmp::max(layout.size(), layout.align());

        let order = match Self::order_for_size(size) {
            Some(order) => order,
            None => return ptr::null_mut(),
        };

        match self.allocate_order(order) {
            Some(addr) => addr as *mut u8,
            None => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || !self.initialized {
            return;
        }

        let size = cmp::max(layout.size(), layout.align());

        let order = match Self::order_for_size(size) {
            Some(order) => order,
            None => return,
        };

        let mut addr = ptr as usize;
        let mut order = order;

        let zone = self
            .zone_for(addr)
            .expect("freed pointer does not belong to any heap zone");

        // Repeatedly try to merge with the buddy block until the buddy is
        // not free or we've reached the top of this block's own zone.
        // Both the buddy address and the merge ceiling are relative to
        // this zone only, so a merge can never cross into another zone.
        while order < zone.order {
            let buddy = match Self::buddy_addr(zone, addr, order) {
                Some(buddy) => buddy,
                None => break,
            };

            if self.remove_free_block(buddy, order) {
                addr = cmp::min(addr, buddy);
                order += 1;
            } else {
                break;
            }
        }

        unsafe {
            self.push_free_block(addr, order);
        }
    }
}
