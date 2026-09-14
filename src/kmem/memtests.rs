//! Kernel memory-management tests.
//!
//! These tests exercise the public heap allocator through the
//! `ALLOCATOR` defined in `kmem::heap`.

use alloc::{boxed::Box, vec::Vec};
 
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator};
 
use crate::test::{test, TestResult};
 
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use crate::kmem::heap::ALLOCATOR;

use super::{
    frame::{align_down, align_up, FRAME_SIZE},
    heap::{buddy::BuddyAllocator, MemoryAllocator},
    FRAME_ALLOCATOR,
};
 
// ============================================================
// Helpers
// ============================================================

/// Allocate memory from the kernel heap.
#[inline]
unsafe fn allocate(
    layout: Layout,
) -> *mut u8 {
    unsafe { ALLOCATOR.alloc(layout) }
}

/// Free memory back to the kernel heap.
#[inline]
unsafe fn deallocate(
    ptr: *mut u8,
    layout: Layout,
) {
    unsafe { ALLOCATOR.dealloc(
        ptr,
        layout,
    ) };
}

// ============================================================
// Basic allocation
// ============================================================

#[test]
fn kmem_allocates_small_block() -> TestResult {
    let layout =
        match Layout::from_size_align(
            8,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct allocation layout",
                );
            }
        };

    let ptr =
        unsafe {
            allocate(layout)
        };

    if ptr.is_null() {
        return TestResult::Fail(
            "small allocation returned null",
        );
    }

    unsafe {
        deallocate(
            ptr,
            layout,
        );
    }

    TestResult::Pass
}

#[test]
fn kmem_allocates_multiple_blocks() -> TestResult {
    let layout =
        match Layout::from_size_align(
            64,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct allocation layout",
                );
            }
        };

    let a =
        unsafe {
            allocate(layout)
        };

    let b =
        unsafe {
            allocate(layout)
        };

    let c =
        unsafe {
            allocate(layout)
        };

    if a.is_null()
        || b.is_null()
        || c.is_null()
    {
        unsafe {
            if !a.is_null() {
                deallocate(
                    a,
                    layout,
                );
            }

            if !b.is_null() {
                deallocate(
                    b,
                    layout,
                );
            }

            if !c.is_null() {
                deallocate(
                    c,
                    layout,
                );
            }
        }

        return TestResult::Fail(
            "one or more allocations returned null",
        );
    }

    if a == b
        || a == c
        || b == c
    {
        unsafe {
            deallocate(
                a,
                layout,
            );
            deallocate(
                b,
                layout,
            );
            deallocate(
                c,
                layout,
            );
        }

        return TestResult::Fail(
            "allocator returned duplicate addresses",
        );
    }

    unsafe {
        deallocate(
            a,
            layout,
        );
        deallocate(
            b,
            layout,
        );
        deallocate(
            c,
            layout,
        );
    }

    TestResult::Pass
}

// ============================================================
// Alignment
// ============================================================

#[test]
fn kmem_respects_alignment() -> TestResult {
    let layouts = [
        (1usize, 1usize),
        (8, 2),
        (16, 4),
        (32, 8),
        (32, 16),
        (64, 32),
        (128, 64),
        (256, 128),
    ];

    for &(size, alignment) in &layouts {
        let layout =
            match Layout::from_size_align(
                size,
                alignment,
            ) {
                Ok(layout) => layout,
                Err(_) => {
                    return TestResult::Fail(
                        "failed to construct alignment layout",
                    );
                }
            };

        let ptr =
            unsafe {
                allocate(layout)
            };

        if ptr.is_null() {
            return TestResult::Fail(
                "aligned allocation returned null",
            );
        }

        if (ptr as usize) % alignment != 0 {
            unsafe {
                deallocate(
                    ptr,
                    layout,
                );
            }

            return TestResult::Fail(
                "allocation does not satisfy requested alignment",
            );
        }

        unsafe {
            deallocate(
                ptr,
                layout,
            );
        }
    }

    TestResult::Pass
}

// ============================================================
// Memory access
// ============================================================

#[test]
fn kmem_allocated_memory_is_writable() -> TestResult {
    let layout =
        match Layout::from_size_align(
            4096,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct layout",
                );
            }
        };

    let ptr =
        unsafe {
            allocate(layout)
        };

    if ptr.is_null() {
        return TestResult::Fail(
            "allocation returned null",
        );
    }

    unsafe {
        for i in 0..4096 {
            ptr.add(i)
                .write((i & 0xff) as u8);
        }

        for i in 0..4096 {
            let value =
                ptr.add(i)
                    .read();

            if value
                != (i & 0xff) as u8
            {
                deallocate(
                    ptr,
                    layout,
                );

                return TestResult::Fail(
                    "memory read-back did not match written data",
                );
            }
        }

        deallocate(
            ptr,
            layout,
        );
    }

    TestResult::Pass
}

// ============================================================
// Free
// ============================================================

#[test]
fn kmem_can_free_memory() -> TestResult {
    let layout =
        match Layout::from_size_align(
            128,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct layout",
                );
            }
        };

    let ptr =
        unsafe {
            allocate(layout)
        };

    if ptr.is_null() {
        return TestResult::Fail(
            "initial allocation failed",
        );
    }

    unsafe {
        deallocate(
            ptr,
            layout,
        );
    }

    let ptr2 =
        unsafe {
            allocate(layout)
        };

    if ptr2.is_null() {
        return TestResult::Fail(
            "allocation after free failed",
        );
    }

    unsafe {
        deallocate(
            ptr2,
            layout,
        );
    }

    TestResult::Pass
}

#[test]
fn kmem_reuses_freed_memory() -> TestResult {
    let layout =
        match Layout::from_size_align(
            256,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct layout",
                );
            }
        };

    let first =
        unsafe {
            allocate(layout)
        };

    if first.is_null() {
        return TestResult::Fail(
            "initial allocation failed",
        );
    }

    unsafe {
        deallocate(
            first,
            layout,
        );
    }

    let second =
        unsafe {
            allocate(layout)
        };

    if second.is_null() {
        return TestResult::Fail(
            "allocation after free failed",
        );
    }

    let reused =
        first == second;

    unsafe {
        deallocate(
            second,
            layout,
        );
    }

    if !reused {
        return TestResult::Fail(
            "allocator did not reuse recently freed memory",
        );
    }

    TestResult::Pass
}

// ============================================================
// Different sizes
// ============================================================

#[test]
fn kmem_handles_different_sizes() -> TestResult {
    let sizes = [
        1usize,
        2,
        4,
        8,
        16,
        32,
        64,
        128,
        256,
        512,
        1024,
    ];

    let mut allocations =
        [ptr::null_mut::<u8>(); 11];

    let mut layouts =
        [None::<Layout>; 11];

    for i in 0..sizes.len() {
        let layout =
            match Layout::from_size_align(
                sizes[i],
                8,
            ) {
                Ok(layout) => layout,
                Err(_) => {
                    return TestResult::Fail(
                        "failed to construct size layout",
                    );
                }
            };

        let allocation =
            unsafe {
                allocate(layout)
            };

        if allocation.is_null() {
            for j in 0..i {
                if !allocations[j].is_null() {
                    unsafe {
                        deallocate(
                            allocations[j],
                            layouts[j].unwrap(),
                        );
                    }
                }
            }

            return TestResult::Fail(
                "allocation failed for one of the requested sizes",
            );
        }

        allocations[i] =
            allocation;

        layouts[i] =
            Some(layout);
    }

    for i in 0..sizes.len() {
        unsafe {
            deallocate(
                allocations[i],
                layouts[i].unwrap(),
            );
        }
    }

    TestResult::Pass
}

// ============================================================
// Many allocations
// ============================================================

#[test]
fn kmem_handles_many_allocations() -> TestResult {
    let layout =
        match Layout::from_size_align(
            64,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct layout",
                );
            }
        };

    let mut allocations =
        [ptr::null_mut::<u8>(); 128];

    for i in 0..allocations.len() {
        let allocation =
            unsafe {
                allocate(layout)
            };

        if allocation.is_null() {
            for ptr in allocations {
                if !ptr.is_null() {
                    unsafe {
                        deallocate(
                            ptr,
                            layout,
                        );
                    }
                }
            }

            return TestResult::Fail(
                "allocator failed during repeated allocation",
            );
        }

        allocations[i] =
            allocation;
    }

    // Ensure every allocation has a unique address.
    for i in 0..allocations.len() {
        for j in (i + 1)..allocations.len() {
            if allocations[i]
                == allocations[j]
            {
                for ptr in allocations {
                    if !ptr.is_null() {
                        unsafe {
                            deallocate(
                                ptr,
                                layout,
                            );
                        }
                    }
                }

                return TestResult::Fail(
                    "allocator returned duplicate addresses",
                );
            }
        }
    }

    for ptr in allocations {
        unsafe {
            deallocate(
                ptr,
                layout,
            );
        }
    }

    TestResult::Pass
}

// ============================================================
// Fragmentation
// ============================================================

#[test]
fn kmem_handles_fragmentation() -> TestResult {
    let layout =
        match Layout::from_size_align(
            128,
            8,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct layout",
                );
            }
        };

    let a =
        unsafe {
            allocate(layout)
        };

    let b =
        unsafe {
            allocate(layout)
        };

    let c =
        unsafe {
            allocate(layout)
        };

    if a.is_null()
        || b.is_null()
        || c.is_null()
    {
        unsafe {
            if !a.is_null() {
                deallocate(
                    a,
                    layout,
                );
            }

            if !b.is_null() {
                deallocate(
                    b,
                    layout,
                );
            }

            if !c.is_null() {
                deallocate(
                    c,
                    layout,
                );
            }
        }

        return TestResult::Fail(
            "initial allocations failed",
        );
    }

    // Free the middle allocation.
    unsafe {
        deallocate(
            b,
            layout,
        );
    }

    let d =
        unsafe {
            allocate(layout)
        };

    if d.is_null() {
        unsafe {
            deallocate(
                a,
                layout,
            );
            deallocate(
                c,
                layout,
            );
        }

        return TestResult::Fail(
            "allocator failed after fragmentation",
        );
    }

    unsafe {
        deallocate(
            a,
            layout,
        );
        deallocate(
            c,
            layout,
        );
        deallocate(
            d,
            layout,
        );
    }

    TestResult::Pass
}

// ============================================================
// Large allocation
// ============================================================

#[test]
fn kmem_allocates_large_block() -> TestResult {
    let layout =
        match Layout::from_size_align(
            16 * 1024,
            16,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct large layout",
                );
            }
        };

    let ptr =
        unsafe {
            allocate(layout)
        };

    if ptr.is_null() {
        return TestResult::Fail(
            "large allocation failed",
        );
    }

    if (ptr as usize) % 16 != 0 {
        unsafe {
            deallocate(
                ptr,
                layout,
            );
        }

        return TestResult::Fail(
            "large allocation has invalid alignment",
        );
    }

    unsafe {
        deallocate(
            ptr,
            layout,
        );
    }

    TestResult::Pass
}

// ============================================================
// High alignment
// ============================================================

#[test]
fn kmem_handles_high_alignment() -> TestResult {
    let layout =
        match Layout::from_size_align(
            128,
            256,
        ) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to construct high-alignment layout",
                );
            }
        };

    let ptr =
        unsafe {
            allocate(layout)
        };

    if ptr.is_null() {
        return TestResult::Fail(
            "high-alignment allocation failed",
        );
    }

    if (ptr as usize) % 256 != 0 {
        unsafe {
            deallocate(
                ptr,
                layout,
            );
        }

        return TestResult::Fail(
            "allocation is not 256-byte aligned",
        );
    }

    unsafe {
        deallocate(
            ptr,
            layout,
        );
    }

    TestResult::Pass
}

#[test]
fn align_up_rounds_up_to_next_multiple() -> TestResult {
    if align_up(0, FRAME_SIZE) != 0 {
        return TestResult::Fail("align_up(0) should be 0");
    }
 
    if align_up(1, FRAME_SIZE) != FRAME_SIZE {
        return TestResult::Fail("align_up(1) should round up to FRAME_SIZE");
    }
 
    if align_up(FRAME_SIZE, FRAME_SIZE) != FRAME_SIZE {
        return TestResult::Fail("align_up of an already-aligned value should not change it");
    }
 
    if align_up(FRAME_SIZE + 1, FRAME_SIZE) != FRAME_SIZE * 2 {
        return TestResult::Fail("align_up should round up past an aligned boundary");
    }
 
    TestResult::Pass
}
 
#[test]
fn align_down_rounds_down_to_previous_multiple() -> TestResult {
    if align_down(0, FRAME_SIZE) != 0 {
        return TestResult::Fail("align_down(0) should be 0");
    }
 
    if align_down(FRAME_SIZE - 1, FRAME_SIZE) != 0 {
        return TestResult::Fail("align_down should round down below the next boundary");
    }
 
    if align_down(FRAME_SIZE, FRAME_SIZE) != FRAME_SIZE {
        return TestResult::Fail("align_down of an already-aligned value should not change it");
    }
 
    if align_down(FRAME_SIZE * 2 - 1, FRAME_SIZE) != FRAME_SIZE {
        return TestResult::Fail("align_down should not overshoot");
    }
 
    TestResult::Pass
}
 
// ============================================================
// Physical frame allocator (live global instance)
// ============================================================
 
#[test]
fn frame_allocation_is_page_aligned() -> TestResult {
    let mut guard = FRAME_ALLOCATOR.lock();
 
    let allocator = match guard.as_mut() {
        Some(allocator) => allocator,
        None => return TestResult::Fail("global frame allocator is not initialized"),
    };
 
    let frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => return TestResult::Fail("allocate_frame returned None (out of memory?)"),
    };
 
    let aligned = frame.start_address().as_u64() % FRAME_SIZE == 0;
 
    unsafe {
        allocator.deallocate_frame(frame);
    }
 
    if aligned {
        TestResult::Pass
    } else {
        TestResult::Fail("allocated frame address was not FRAME_SIZE-aligned")
    }
}
 
#[test]
fn consecutive_allocations_are_distinct() -> TestResult {
    let mut guard = FRAME_ALLOCATOR.lock();
 
    let allocator = match guard.as_mut() {
        Some(allocator) => allocator,
        None => return TestResult::Fail("global frame allocator is not initialized"),
    };
 
    let a = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("first allocate_frame returned None"),
    };
 
    let b = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("second allocate_frame returned None"),
    };
 
    let distinct = a.start_address() != b.start_address();
 
    unsafe {
        allocator.deallocate_frame(a);
        allocator.deallocate_frame(b);
    }
 
    if distinct {
        TestResult::Pass
    } else {
        TestResult::Fail("two consecutive allocations returned the same frame")
    }
}
 
#[test]
fn free_list_reuses_freed_frames_in_lifo_order() -> TestResult {
    let mut guard = FRAME_ALLOCATOR.lock();
 
    let allocator = match guard.as_mut() {
        Some(allocator) => allocator,
        None => return TestResult::Fail("global frame allocator is not initialized"),
    };
 
    let a = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("allocate_frame returned None"),
    };
 
    let b = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("allocate_frame returned None"),
    };
 
    // Free a, then b. The free list is a LIFO stack, so b should come
    // back out first -- this directly exercises push_free_frame /
    // pop_free_frame ordering.
    unsafe {
        allocator.deallocate_frame(a);
        allocator.deallocate_frame(b);
    }
 
    let first = allocator.allocate_frame();
    let second = allocator.allocate_frame();
 
    let (first, second) = match (first, second) {
        (Some(first), Some(second)) => (first, second),
        _ => return TestResult::Fail("allocate_frame returned None after freeing two frames"),
    };
 
    let correct_order =
        first.start_address() == b.start_address() && second.start_address() == a.start_address();
 
    // Restore state regardless of outcome so the test never leaks frames.
    unsafe {
        allocator.deallocate_frame(first);
        allocator.deallocate_frame(second);
    }
 
    if correct_order {
        TestResult::Pass
    } else {
        TestResult::Fail("freed frames were not reused in LIFO order")
    }
}
 
#[test]
fn free_frame_count_reflects_deallocations() -> TestResult {
    let mut guard = FRAME_ALLOCATOR.lock();
 
    let allocator = match guard.as_mut() {
        Some(allocator) => allocator,
        None => return TestResult::Fail("global frame allocator is not initialized"),
    };
 
    let before = allocator.free_frame_count();
 
    let a = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("allocate_frame returned None"),
    };
 
    let b = match allocator.allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("allocate_frame returned None"),
    };
 
    // Two of the frames we just took might themselves have come from the
    // free list (if earlier tests or kernel activity left some there), so
    // don't assume `before` was zero -- just that freeing exactly two
    // frames grows the list by exactly two relative to its state right
    // before we freed them.
    let before_dealloc = allocator.free_frame_count();
 
    unsafe {
        allocator.deallocate_frame(a);
        allocator.deallocate_frame(b);
    }
 
    let after = allocator.free_frame_count();
 
    let _ = before; // kept for readability of intent above
 
    if after == before_dealloc + 2 {
        TestResult::Pass
    } else {
        TestResult::Fail("free_frame_count did not increase by exactly 2 after two deallocations")
    }
}
 
// ============================================================
// Buddy allocator (private, self-contained scratch heaps)
// ============================================================
 
/// A correctly-aligned scratch heap used only by these tests. 64-byte
/// alignment satisfies the buddy allocator's MIN_ORDER requirement.
#[repr(align(64))]
struct TestHeap<const N: usize>([u8; N]);
 
/// 64 KiB, a power of two -> decomposes into a single zone.
static mut BUDDY_TEST_HEAP: TestHeap<65536> = TestHeap([0; 65536]);

/// A 32 KiB-aligned scratch heap used to test buddy-zone boundaries.
#[repr(align(32768))]
struct BuddyZonedTestHeap([u8; 48 * 1024]);

/// 48 KiB, not a power of two -> decomposes into a 32 KiB zone and a
/// 16 KiB zone. Used specifically to exercise the zone-boundary fix.
static mut BUDDY_ZONED_TEST_HEAP: BuddyZonedTestHeap =
    BuddyZonedTestHeap([0; 48 * 1024]);

#[test]
fn buddy_alloc_dealloc_roundtrip() -> TestResult {
    let mut allocator = BuddyAllocator::new();
    let heap_start = &raw mut BUDDY_TEST_HEAP as usize;
 
    unsafe {
        allocator.init(heap_start, 65536);
    }
 
    let layout = match core::alloc::Layout::from_size_align(128, 8) {
        Ok(layout) => layout,
        Err(_) => return TestResult::Fail("failed to build test layout"),
    };
 
    let ptr = unsafe { allocator.alloc(layout) };
 
    if ptr.is_null() {
        return TestResult::Fail("alloc returned null for a small in-range layout");
    }
 
    let aligned = (ptr as usize) % layout.align() == 0;
 
    unsafe {
        allocator.dealloc(ptr, layout);
    }
 
    if aligned {
        TestResult::Pass
    } else {
        TestResult::Fail("returned pointer was not aligned to the requested alignment")
    }
}
 
#[test]
fn buddy_freeing_everything_fully_reclaims_the_heap() -> TestResult {
    let mut allocator = BuddyAllocator::new();
    let heap_start = &raw mut BUDDY_TEST_HEAP as usize;
 
    unsafe {
        allocator.init(heap_start, 65536);
    }
 
    let layout = match core::alloc::Layout::from_size_align(4096, 8) {
        Ok(layout) => layout,
        Err(_) => return TestResult::Fail("failed to build test layout"),
    };
 
    // Allocate four blocks, then free them out of order to force several
    // buddy merges rather than one tidy reverse-order collapse.
    let a = unsafe { allocator.alloc(layout) };
    let b = unsafe { allocator.alloc(layout) };
    let c = unsafe { allocator.alloc(layout) };
    let d = unsafe { allocator.alloc(layout) };
 
    if a.is_null() || b.is_null() || c.is_null() || d.is_null() {
        return TestResult::Fail("expected four 4 KiB allocations to fit in a 64 KiB heap");
    }
 
    unsafe {
        allocator.dealloc(c, layout);
        allocator.dealloc(a, layout);
        allocator.dealloc(d, layout);
        allocator.dealloc(b, layout);
    }
 
    // If coalescing is correct, the heap should have recombined back into
    // one large free block, so a request for half the heap should now
    // succeed even though no single prior allocation was that big.
    let big_layout = match core::alloc::Layout::from_size_align(32768, 8) {
        Ok(layout) => layout,
        Err(_) => return TestResult::Fail("failed to build test layout"),
    };
 
    let big = unsafe { allocator.alloc(big_layout) };
    let reclaimed = !big.is_null();
 
    if !big.is_null() {
        unsafe { allocator.dealloc(big, big_layout) };
    }
 
    if reclaimed {
        TestResult::Pass
    } else {
        TestResult::Fail("heap did not fully recombine after freeing everything (coalescing bug)")
    }
}
 
#[test]
fn buddy_oversized_allocation_fails_cleanly() -> TestResult {
    let mut allocator = BuddyAllocator::new();
    let heap_start = &raw mut BUDDY_TEST_HEAP as usize;
 
    unsafe {
        allocator.init(heap_start, 65536);
    }
 
    // Ask for more than the entire test heap.
    let layout = match core::alloc::Layout::from_size_align(1024 * 1024, 8) {
        Ok(layout) => layout,
        Err(_) => return TestResult::Fail("failed to build test layout"),
    };
 
    let ptr = unsafe { allocator.alloc(layout) };
 
    if ptr.is_null() {
        TestResult::Pass
    } else {
        TestResult::Fail("alloc should have returned null for a layout larger than the heap")
    }
}
 
#[test]
fn buddy_live_allocations_do_not_overlap() -> TestResult {
    let mut allocator = BuddyAllocator::new();
    let heap_start = &raw mut BUDDY_TEST_HEAP as usize;
 
    unsafe {
        allocator.init(heap_start, 65536);
    }
 
    let layout = match core::alloc::Layout::from_size_align(256, 8) {
        Ok(layout) => layout,
        Err(_) => return TestResult::Fail("failed to build test layout"),
    };
 
    let a = unsafe { allocator.alloc(layout) };
    let b = unsafe { allocator.alloc(layout) };
    let c = unsafe { allocator.alloc(layout) };
 
    if a.is_null() || b.is_null() || c.is_null() {
        return TestResult::Fail("expected three small allocations to succeed");
    }
 
    // Stamp each block with a distinct byte pattern. If two "distinct"
    // allocations actually overlapped, one write would clobber another.
    unsafe {
        core::ptr::write_bytes(a, 0xAA, layout.size());
        core::ptr::write_bytes(b, 0xBB, layout.size());
        core::ptr::write_bytes(c, 0xCC, layout.size());
    }
 
    let a_ok = (0..layout.size()).all(|i| unsafe { *a.add(i) } == 0xAA);
    let b_ok = (0..layout.size()).all(|i| unsafe { *b.add(i) } == 0xBB);
    let c_ok = (0..layout.size()).all(|i| unsafe { *c.add(i) } == 0xCC);
 
    unsafe {
        allocator.dealloc(a, layout);
        allocator.dealloc(b, layout);
        allocator.dealloc(c, layout);
    }
 
    if a_ok && b_ok && c_ok {
        TestResult::Pass
    } else {
        TestResult::Fail("two or more live allocations overlapped in memory")
    }
}


#[test]
fn buddy_zones_do_not_corrupt_each_other() -> TestResult {
    let mut allocator = BuddyAllocator::new();

    let heap_start =
        unsafe {
            &raw mut BUDDY_ZONED_TEST_HEAP.0 as *mut u8 as usize
        };

    // The test specifically requires the heap to begin on a 32 KiB
    // boundary so the allocator can form a 32 KiB zone followed by
    // a 16 KiB zone.
    if heap_start % (32 * 1024) != 0 {
        return TestResult::Fail(
            "test heap is not 32 KiB aligned",
        );
    }

    unsafe {
        allocator.init(
            heap_start,
            48 * 1024,
        );
    }

    if allocator.zone_count != 2 {
        return TestResult::Fail(
            "48 KiB heap should be decomposed into exactly two zones",
        );
    }

    let first_zone =
        match allocator.zones[0] {
            Some(zone) => zone,
            None => {
                return TestResult::Fail(
                    "first zone is missing",
                );
            }
        };

    let second_zone =
        match allocator.zones[1] {
            Some(zone) => zone,
            None => {
                return TestResult::Fail(
                    "second zone is missing",
                );
            }
        };

    if first_zone.base != heap_start {
        return TestResult::Fail(
            "first zone does not start at heap base",
        );
    }

    if first_zone.order != 15 {
        return TestResult::Fail(
            "first zone should be 32 KiB",
        );
    }

    if second_zone.base != heap_start + 32 * 1024 {
        return TestResult::Fail(
            "second zone should begin after the 32 KiB zone",
        );
    }

    if second_zone.order != 14 {
        return TestResult::Fail(
            "second zone should be 16 KiB",
        );
    }

    // Three 16 KiB allocations must fit:
    //
    //   [ 16 KiB ][ 16 KiB ][ 16 KiB ]
    //   <----32 KiB----> <16 KiB>
    //
    // This proves both zones participate in allocation.
    let layout_16k =
        match Layout::from_size_align(16 * 1024, 8) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to create 16 KiB layout",
                );
            }
        };

    let first =
        unsafe { allocator.alloc(layout_16k) };

    let second =
        unsafe { allocator.alloc(layout_16k) };

    let third =
        unsafe { allocator.alloc(layout_16k) };

    if first.is_null()
        || second.is_null()
        || third.is_null()
    {
        if !first.is_null() {
            unsafe {
                allocator.dealloc(
                    first,
                    layout_16k,
                );
            }
        }

        if !second.is_null() {
            unsafe {
                allocator.dealloc(
                    second,
                    layout_16k,
                );
            }
        }

        if !third.is_null() {
            unsafe {
                allocator.dealloc(
                    third,
                    layout_16k,
                );
            }
        }

        return TestResult::Fail(
            "expected three 16 KiB blocks to fit across the 32 KiB + 16 KiB zones",
        );
    }

    // Free the two blocks belonging to the large zone first.
    //
    // Depending on allocation order, first/second should occupy the
    // 32 KiB zone, while third should occupy the 16 KiB zone.
    unsafe {
        allocator.dealloc(
            second,
            layout_16k,
        );

        allocator.dealloc(
            first,
            layout_16k,
        );

        allocator.dealloc(
            third,
            layout_16k,
        );
    }

    // The large zone must have been independently reconstructed.
    let layout_32k =
        match Layout::from_size_align(32 * 1024, 8) {
            Ok(layout) => layout,
            Err(_) => {
                return TestResult::Fail(
                    "failed to create 32 KiB layout",
                );
            }
        };

    let reclaimed =
        unsafe { allocator.alloc(layout_32k) };

    if reclaimed.is_null() {
        return TestResult::Fail(
            "could not reclaim the complete 32 KiB zone",
        );
    }

    if reclaimed as usize != first_zone.base {
        unsafe {
            allocator.dealloc(
                reclaimed,
                layout_32k,
            );
        }

        return TestResult::Fail(
            "32 KiB allocation did not come from the large zone",
        );
    }

    unsafe {
        allocator.dealloc(
            reclaimed,
            layout_32k,
        );
    }

    // The neighboring 16 KiB zone must still work independently.
    let small =
        unsafe { allocator.alloc(layout_16k) };

    if small.is_null() {
        return TestResult::Fail(
            "16 KiB zone was lost or corrupted",
        );
    }

    let small_zone =
        match allocator.zone_for(small as usize) {
            Some(zone) => zone,
            None => {
                unsafe {
                    allocator.dealloc(
                        small,
                        layout_16k,
                    );
                }

                return TestResult::Fail(
                    "small allocation is outside all zones",
                );
            }
        };

    if small_zone.base != second_zone.base
        || small_zone.order != second_zone.order
    {
        unsafe {
            allocator.dealloc(
                small,
                layout_16k,
            );
        }

        return TestResult::Fail(
            "16 KiB allocation came from the wrong zone",
        );
    }

    unsafe {
        allocator.dealloc(
            small,
            layout_16k,
        );
    }

    TestResult::Pass
}

// ============================================================
// Kernel heap integration (GlobalAlloc via the `alloc` crate)
// ============================================================
 
#[test]
fn heap_vec_push_and_grow() -> TestResult {
    let mut v: Vec<u32> = Vec::new();
 
    for i in 0..256u32 {
        v.push(i);
    }
 
    if v.len() != 256 {
        return TestResult::Fail("Vec length did not match the number of pushes");
    }
 
    if v[0] != 0 || v[255] != 255 {
        return TestResult::Fail("Vec contents were corrupted");
    }
 
    drop(v);
 
    TestResult::Pass
}
 
#[test]
fn heap_box_allocation_and_drop() -> TestResult {
    let boxed = Box::new([0xABu8; 4096]);
 
    let all_correct = boxed.iter().all(|&b| b == 0xAB);
 
    drop(boxed);
 
    if all_correct {
        TestResult::Pass
    } else {
        TestResult::Fail("Box contents did not match what was written")
    }
}
 
#[test]
fn heap_repeated_alloc_dealloc_does_not_exhaust_heap() -> TestResult {
    // A basic leak/fragmentation smoke test: many short-lived allocations
    // should not gradually eat the heap if dealloc is wired up correctly.
    for _ in 0..64 {
        let v: Vec<u8> = alloc::vec![0u8; 8192];
        drop(v);
    }
 
    TestResult::Pass
}
