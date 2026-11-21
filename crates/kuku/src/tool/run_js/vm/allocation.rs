use std::alloc::{self, Layout};
use std::mem;
use std::ptr;

use rquickjs::allocator::Allocator;

const MAPPED_ALLOCATION_BYTES: usize = 128 * 1024;
const HEADER_BYTES: usize = mem::size_of::<AllocationHeader>();

pub(super) struct ScriptAllocator;

#[repr(align(16))]
struct AllocationHeader {
    layout: Layout,
}

// SAFETY: Every non-null payload is aligned and points past an initialized
// header in its own allocation. The stored layout identifies the backing
// allocation and its usable size. Resize preserves the original on failure.
unsafe impl Allocator for ScriptAllocator {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        allocate(size, false)
    }

    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        count
            .checked_mul(size)
            .map_or(ptr::null_mut(), |bytes| allocate(bytes, true))
    }

    unsafe fn dealloc(&mut self, payload: *mut u8) {
        // SAFETY: QuickJS passes a live payload created by this allocator.
        let (base, layout) = unsafe { read_header(payload) };
        if uses_pages(layout) {
            // SAFETY: The stored layout covers the original entire mapping.
            unsafe { unmap(base, layout.size()) };
        } else {
            // SAFETY: This base and layout were produced by the Rust allocator.
            unsafe { alloc::dealloc(base, layout) };
        }
    }

    unsafe fn realloc(&mut self, payload: *mut u8, size: usize) -> *mut u8 {
        if payload.is_null() {
            return self.alloc(size);
        }
        if size == 0 {
            // SAFETY: The caller supplied a live allocation to dispose of.
            unsafe { self.dealloc(payload) };
            return ptr::null_mut();
        }
        let Some(new_layout) = allocation_layout(size) else {
            return ptr::null_mut();
        };
        // SAFETY: QuickJS passes a live payload created by this allocator.
        let (base, old_layout) = unsafe { read_header(payload) };
        if old_layout == new_layout {
            return payload;
        }
        if !uses_pages(old_layout) && !uses_pages(new_layout) {
            // SAFETY: The original base uses the stored Rust layout and the new
            // size was validated for the same alignment.
            let base = unsafe { alloc::realloc(base, old_layout, new_layout.size()) };
            // SAFETY: A non-null realloc result has new_layout and a null
            // result leaves the original header untouched.
            return unsafe { write_header(base, new_layout) };
        }
        let replacement = self.alloc(size);
        if !replacement.is_null() {
            let preserved = (old_layout.size() - HEADER_BYTES).min(size);
            // SAFETY: The replacement is separate, both payloads cover the
            // copied range, and QuickJS gives exclusive access during resize.
            unsafe {
                ptr::copy_nonoverlapping(payload, replacement, preserved);
                self.dealloc(payload);
            };
        }
        replacement
    }

    unsafe fn usable_size(payload: *mut u8) -> usize {
        // SAFETY: QuickJS queries live payloads created by this allocator.
        let (_, layout) = unsafe { read_header(payload) };
        layout.size() - HEADER_BYTES
    }
}

fn allocate(size: usize, zeroed: bool) -> *mut u8 {
    let Some(layout) = allocation_layout(size) else {
        return ptr::null_mut();
    };
    let base = if uses_pages(layout) {
        map(layout.size())
    } else {
        // SAFETY: The checked layout is nonzero and uses the header alignment.
        unsafe {
            if zeroed {
                alloc::alloc_zeroed(layout)
            } else {
                alloc::alloc(layout)
            }
        }
    };
    // SAFETY: Both backing allocation paths return null or a block satisfying
    // the size and alignment of layout.
    unsafe { write_header(base, layout) }
}

fn allocation_layout(size: usize) -> Option<Layout> {
    if size == 0 {
        return None;
    }
    Layout::from_size_align(
        size.checked_add(HEADER_BYTES)?,
        mem::align_of::<AllocationHeader>(),
    )
    .ok()
}

fn uses_pages(layout: Layout) -> bool {
    layout.size() - HEADER_BYTES >= MAPPED_ALLOCATION_BYTES
}

unsafe fn write_header(base: *mut u8, layout: Layout) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    // SAFETY: The backing allocator provides a suitably aligned block with room
    // for the header and payload. The header stays private to this allocator.
    unsafe {
        base.cast::<AllocationHeader>()
            .write(AllocationHeader { layout });
        base.add(HEADER_BYTES)
    }
}

unsafe fn read_header(payload: *mut u8) -> (*mut u8, Layout) {
    // SAFETY: The caller ensures that payload comes from write_header and still
    // belongs to a live allocation, so the initialized header is in bounds.
    unsafe {
        let base = payload.sub(HEADER_BYTES);
        (base, (*base.cast::<AllocationHeader>()).layout)
    }
}

#[cfg(unix)]
fn map(size: usize) -> *mut u8 {
    // SAFETY: The checked nonzero size is requested as private, anonymous,
    // zero-filled pages. The OS chooses the address and page alignment.
    let mapped = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        ptr::null_mut()
    } else {
        mapped.cast()
    }
}

#[cfg(unix)]
unsafe fn unmap(base: *mut u8, size: usize) {
    // SAFETY: The base and size identify the exact anonymous mapping being freed.
    unsafe { libc::munmap(base.cast(), size) };
}

#[cfg(windows)]
fn map(size: usize) -> *mut u8 {
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
    };

    // SAFETY: The OS chooses the page-aligned block and zero-initializes the
    // private pages committed by this nonzero request.
    unsafe { VirtualAlloc(ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }.cast()
}

#[cfg(windows)]
unsafe fn unmap(base: *mut u8, _: usize) {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};

    // SAFETY: This is the original reservation address. MEM_RELEASE with zero
    // size releases the entire reservation.
    unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_allocation_classes_are_aligned_and_calloc_zeroes_the_payload() {
        let mut allocator = ScriptAllocator;
        for size in [2, MAPPED_ALLOCATION_BYTES] {
            let payload = allocator.calloc(1, size);
            assert!(!payload.is_null());
            assert_eq!(0, payload.align_offset(mem::align_of::<AllocationHeader>()));
            // SAFETY: The allocation is live and calloc initialized its range.
            unsafe {
                assert_eq!(size, ScriptAllocator::usable_size(payload));
                assert!(std::slice::from_raw_parts(payload, size)
                    .iter()
                    .all(|&byte| byte == 0));
                allocator.dealloc(payload);
            }
        }
        assert!(allocator.calloc(usize::MAX, 2).is_null());
        assert!(allocator.alloc(usize::MAX).is_null());
    }

    #[test]
    fn realloc_preserves_bytes_across_allocation_classes_and_on_failure() {
        let mut allocator = ScriptAllocator;
        let mut payload = allocator.alloc(16);
        assert!(!payload.is_null());
        // SAFETY: Every successful allocation and resize provides at least 16
        // bytes, and the live pointer is freed exactly once.
        unsafe {
            payload.write_bytes(0x5a, 16);
            for size in [32, MAPPED_ALLOCATION_BYTES, MAPPED_ALLOCATION_BYTES * 2, 16] {
                payload = allocator.realloc(payload, size);
                assert!(!payload.is_null());
                assert_eq!(size, ScriptAllocator::usable_size(payload));
                assert_eq!(&[0x5a; 16], std::slice::from_raw_parts(payload, 16));
                assert!(allocator.realloc(payload, usize::MAX).is_null());
                assert_eq!(&[0x5a; 16], std::slice::from_raw_parts(payload, 16));
            }
            assert!(allocator.realloc(payload, 0).is_null());
        }
    }
}
