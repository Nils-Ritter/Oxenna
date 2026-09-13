//! Kernel heap allocator.
//!
//! The heap consists of three layers:
//!
//! ```text
//!     Kernel allocation
//!            |
//!            v
//!     Buddy allocator
//!            |
//!            v
//!     Virtual heap pages
//!            |
//!            v
//!     Physical frames
//! ```

use core::alloc::{
    GlobalAlloc,
    Layout,
};

use spin::Mutex;

use x86_64::{
    structures::paging::{
        FrameAllocator,
        mapper::MapToError,
        Mapper,
        Page,
        PageTableFlags,
        Size4KiB,
    },
    VirtAddr,
};

use crate::kmem::heap::buddy::BuddyAllocator;

pub mod buddy;

/// Common interface implemented by kernel heap allocators.
pub trait MemoryAllocator {
    /// Allocate memory satisfying `layout`, or return a null pointer on failure.
    ///
    /// # Safety
    ///
    /// The allocator must have been initialized, and the caller must later
    /// pass the returned pointer (and the same `layout`) to `dealloc`.
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;

    /// Deallocate a block previously returned by `alloc` with the same `layout`.
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by this allocator's `alloc` with the
    /// same `layout`, and must not be used again after this call.
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}

/// Start address of the kernel heap.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Initial kernel heap size.
///
/// This can be increased later or replaced with a dynamically growing heap.
pub const HEAP_SIZE: usize = 1024 * 1024 * 100; // 100 MiB

/// Global kernel allocator.
#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::new();

/// Initialize the kernel heap.
///
/// This maps the heap's virtual address range to physical frames and then
/// initializes the buddy allocator.
///
/// # Safety
///
/// The caller must guarantee that:
///
/// - `mapper` is a valid page-table mapper.
/// - `frame_allocator` returns valid unused physical frames.
/// - The heap virtual-address range is not already in use.
/// - The mapped frames are exclusively owned by the heap.
pub unsafe fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let heap_start = VirtAddr::new(HEAP_START as u64);
    let heap_end = heap_start + (HEAP_SIZE as u64) - 1;

    let heap_start_page = Page::containing_address(heap_start);
    let heap_end_page = Page::containing_address(heap_end);

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    for page in Page::range_inclusive(heap_start_page, heap_end_page) {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;

        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    unsafe {
        ALLOCATOR.inner.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

/// Thread-safe wrapper around the heap allocator.
pub struct LockedHeap {
    inner: Mutex<BuddyAllocator>,
}

impl LockedHeap {
    /// Create an uninitialized heap allocator.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(BuddyAllocator::new()),
        }
    }

    /// Initialize the heap allocator.
    ///
    /// # Safety
    ///
    /// The supplied memory range must be valid writable memory and must not
    /// overlap any existing allocation.
    #[allow(unused)]
    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        unsafe {
            self.inner.lock().init(heap_start, heap_size);
        }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.inner.lock().alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            self.inner.lock().dealloc(ptr, layout);
        }
    }
}
