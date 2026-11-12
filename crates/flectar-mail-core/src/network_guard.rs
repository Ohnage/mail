//! Counts outbound network entry points so startup/tab tests can assert zero.
//!
//! Tests may call [`forbid`] to panic on the next network attempt. Production
//! never forbids; it only increments the counter for diagnostics.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static CALLS: AtomicU64 = AtomicU64::new(0);
static FORBIDDEN: AtomicBool = AtomicBool::new(false);

/// Record one outbound network attempt (JMAP/IMAP/HTTP/OAuth/Gmail).
pub fn record(entry: &'static str) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    if FORBIDDEN.load(Ordering::Relaxed) {
        panic!("network forbidden during local-only path: {entry}");
    }
}

pub fn count() -> u64 {
    CALLS.load(Ordering::Relaxed)
}

pub fn reset() {
    CALLS.store(0, Ordering::Relaxed);
}

pub fn forbid(on: bool) {
    FORBIDDEN.store(on, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_is_monotonic_until_reset() {
        reset();
        forbid(false);
        let before = count();
        record("test");
        assert_eq!(count(), before + 1);
        reset();
        assert_eq!(count(), 0);
    }
}
