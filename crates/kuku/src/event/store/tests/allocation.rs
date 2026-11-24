use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use super::{EventPayload, EventStore, payload};

const MIB: usize = 1024 * 1024;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

struct TrackingAllocator;

// SAFETY: GlobalAlloc is the only allocator hook; System retains ownership and const TLS only records sizes.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: GlobalAlloc supplies a valid layout, and System owns the returned allocation.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc requires this pointer and layout to identify a live System allocation.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);
        // SAFETY: GlobalAlloc supplies the original System allocation and a valid replacement size.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn record_allocation(size: usize) {
    let _ = TRACKING.try_with(|tracking| {
        if tracking.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        }
    });
}

#[test]
fn oversized_known_and_unknown_payloads_do_not_allocate_another_input_sized_copy() {
    let temp = tempfile::tempdir().unwrap();
    for known in [false, true] {
        for size in [2 * MIB, 32 * MIB] {
            let path = temp.path().join(format!("events-{known}-{size}.jsonl"));
            let mut store = EventStore::open(&path).unwrap();
            let text = "x".repeat(size);
            let event = if known {
                EventPayload::MessageUser {
                    ts: String::new(),
                    conversation: "main".to_string(),
                    turn: 1,
                    text,
                    from: None,
                    job_delivery: false,
                }
            } else {
                EventPayload::Unknown(serde_json::json!({
                    "kind": "runtime.test", "text": text,
                }))
            };

            LARGEST.with(|largest| largest.set(0));
            TRACKING.with(|tracking| tracking.set(true));
            let result = store.append(event);
            TRACKING.with(|tracking| tracking.set(false));
            let largest = LARGEST.with(Cell::get);

            assert!(matches!(
                result,
                Err(crate::error::Error::InvalidArgument(_))
            ));
            assert!(
                largest <= MIB,
                "known={known}, input={size}, allocation={largest}"
            );
            assert_eq!(0, store.last_id());
            assert_eq!(0, std::fs::metadata(&path).unwrap().len());
            assert_eq!(1, store.append(payload()).unwrap().id);
        }
    }
}
