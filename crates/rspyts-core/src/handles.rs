//! The slab behind opaque class handles (ABI §8).
//!
//! Each `#[bridge] impl` expansion declares one
//! `static SLAB: Slab<TheType> = Slab::new();`. Handles are `u64`,
//! monotonically increasing from 1, never reused, and enforced below 2^53
//! ([`MAX_HANDLE`]) so they survive JSON and JS `number` transport.

use crate::error::BridgeError;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Handles must survive JSON and JS `number` transport, so they may never
/// reach 2^53 (ABI §8). Enforced in [`Slab::insert`]; at one allocation
/// per nanosecond the bound is ~104 days short of 300 years away, so the
/// panic (surfacing as a status-2 envelope) is a correctness statement,
/// not an expected event.
pub const MAX_HANDLE: u64 = 1 << 53;

pub struct Slab<T> {
    // BTreeMap because its `new` is const, sparing a OnceLock dance.
    entries: Mutex<BTreeMap<u64, Arc<Mutex<T>>>>,
    next: AtomicU64,
}

impl<T> Slab<T> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Test-only constructor starting the counter near an arbitrary point,
    /// so the exhaustion bound is actually testable.
    #[cfg(test)]
    const fn starting_at(next: u64) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(next),
        }
    }

    /// Store `value` and return its handle.
    pub fn insert(&self, value: T) -> u64 {
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        assert!(
            handle < MAX_HANDLE,
            "rspyts: handle space exhausted (2^53 handles allocated)"
        );
        self.entries
            .lock()
            .expect("rspyts: slab poisoned")
            .insert(handle, Arc::new(Mutex::new(value)));
        handle
    }

    fn get(&self, handle: u64) -> Result<Arc<Mutex<T>>, BridgeError> {
        self.entries
            .lock()
            .expect("rspyts: slab poisoned")
            .get(&handle)
            .cloned()
            .ok_or_else(BridgeError::stale_handle)
    }

    /// Run `f` with shared access to the object behind `handle`.
    ///
    /// The per-object lock is held for the duration of the call, so
    /// concurrent method calls on one handle serialize (ABI §8). The slab
    /// lock itself is NOT held during `f`.
    ///
    /// Poisoning is deliberately ignored: a panic inside a method has
    /// already been reported to the caller as a status-2 envelope, and the
    /// object may be in any state the user code left it in — exactly like
    /// an object surviving an exception. It must remain usable and, above
    /// all, droppable.
    pub fn with<R>(&self, handle: u64, f: impl FnOnce(&T) -> R) -> Result<R, BridgeError> {
        let entry = self.get(handle)?;
        let guard = entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(f(&guard))
    }

    /// Run `f` with exclusive access to the object behind `handle`.
    pub fn with_mut<R>(&self, handle: u64, f: impl FnOnce(&mut T) -> R) -> Result<R, BridgeError> {
        let entry = self.get(handle)?;
        let mut guard = entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(f(&mut guard))
    }

    /// Drop the object behind `handle`. Idempotent: unknown handles are
    /// ignored (ABI §8 — `__drop` must be safe to call from `__del__`,
    /// finalizers, and explicit `close()` in any order).
    pub fn remove(&self, handle: u64) {
        let entry = self
            .entries
            .lock()
            .expect("rspyts: slab poisoned")
            .remove(&handle);
        // Dropped here, outside the slab lock, in case T::drop is slow.
        drop(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_with_remove_lifecycle() {
        static SLAB: Slab<Vec<i32>> = Slab::new();
        let h = SLAB.insert(vec![1, 2]);
        assert!(h >= 1);
        SLAB.with_mut(h, |v| v.push(3)).unwrap();
        let len = SLAB.with(h, |v| v.len()).unwrap();
        assert_eq!(len, 3);
        SLAB.remove(h);
        SLAB.remove(h); // idempotent
        let err = SLAB.with(h, |_| ()).unwrap_err();
        assert_eq!(err.code, crate::error::codes::STALE_HANDLE);
    }

    #[test]
    fn insert_panics_at_the_transport_bound() {
        static SLAB: Slab<u8> = Slab::starting_at(MAX_HANDLE - 1);
        assert_eq!(SLAB.insert(0), MAX_HANDLE - 1);
        let exhausted = std::panic::catch_unwind(|| SLAB.insert(0));
        assert!(exhausted.is_err());
        SLAB.remove(MAX_HANDLE - 1);
    }

    #[test]
    fn handles_are_never_zero_and_never_reused() {
        static SLAB: Slab<u8> = Slab::new();
        let a = SLAB.insert(0);
        SLAB.remove(a);
        let b = SLAB.insert(0);
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn concurrent_hammer_stays_consistent() {
        static SLAB: Slab<u64> = Slab::new();
        const THREADS: u64 = 8;
        const OPS: u64 = 1000;

        let all: Vec<u64> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..THREADS)
                .map(|t| {
                    scope.spawn(move || {
                        let mut seen = Vec::with_capacity(OPS as usize);
                        for i in 0..OPS {
                            let h = SLAB.insert(t);
                            SLAB.with_mut(h, |v| *v += i).unwrap();
                            assert_eq!(SLAB.with(h, |v| *v).unwrap(), t + i);
                            SLAB.remove(h);
                            seen.push(h);
                        }
                        // Monotonic per thread: fetch_add never hands the
                        // same thread a smaller handle, even under load.
                        assert!(seen.windows(2).all(|w| w[0] < w[1]));
                        seen
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|w| w.join().unwrap())
                .collect()
        });

        // No handle was ever reused across threads, and allocation was
        // gapless: exactly 1..=8000 was handed out.
        let unique: std::collections::BTreeSet<u64> = all.iter().copied().collect();
        assert_eq!(unique.len(), (THREADS * OPS) as usize);
        assert_eq!(unique.first().copied(), Some(1));
        assert_eq!(unique.last().copied(), Some(THREADS * OPS));

        // Every thread removed what it inserted: the slab drained fully.
        assert!(SLAB.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn with_serializes_against_in_flight_with_mut() {
        static SLAB: Slab<i32> = Slab::new();
        let h = SLAB.insert(0);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                SLAB.with_mut(h, |v| {
                    locked_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    *v = 42;
                })
                .unwrap();
            });
            // The writer provably holds the object lock before the reader
            // even starts, so the reader must block until *v == 42; seeing
            // the initial 0 would mean with() skipped the object lock.
            locked_rx.recv().unwrap();
            let reader = scope.spawn(move || SLAB.with(h, |v| *v).unwrap());
            release_tx.send(()).unwrap();
            assert_eq!(reader.join().unwrap(), 42);
            writer.join().unwrap();
        });
        SLAB.remove(h);
    }

    #[test]
    fn remove_mid_use_keeps_object_alive() {
        static SLAB: Slab<Vec<i32>> = Slab::new();
        let h = SLAB.insert(vec![1]);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            let user = scope.spawn(move || {
                SLAB.with_mut(h, |v| {
                    locked_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    v.push(2);
                    v.clone()
                })
                .unwrap()
            });
            locked_rx.recv().unwrap();
            // Remove while the user holds the object lock: the map entry
            // goes away, but the Arc keeps the object alive for the user.
            SLAB.remove(h);
            resume_tx.send(()).unwrap();
            assert_eq!(user.join().unwrap(), vec![1, 2]);
        });

        let err = SLAB.with(h, |_| ()).unwrap_err();
        assert_eq!(err.code, crate::error::codes::STALE_HANDLE);
    }
}
