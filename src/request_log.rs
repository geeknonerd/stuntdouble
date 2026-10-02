//! Shared per-request call-chain evidence for the structured request log.
//! Contract: docs/contracts/cli.md (request logging)
//!
//! Upstream and file calls record the same shape of evidence (begin one entry,
//! update it as the call progresses, finalize it with timing, snapshot the
//! chain for the log line) through near-identical interfaces. This module owns
//! that behavior once: timing math, the begin/update/snapshot container, and
//! the late-finalize guard for streamed bodies. Each record kind keeps its own
//! fields and log shape; only the behavior is unified.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

/// Milliseconds with two decimals, shared by every request-log timing.
#[must_use]
pub fn elapsed_ms(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

/// One entry in a per-request call chain. Each record kind keeps its own
/// fields and log shape; the chain only needs timing, bytes, and rendering.
pub trait LogRecord {
    /// Log shape for the structured request line.
    fn to_json(&self) -> Json;
    /// Record the finalized call duration.
    fn set_duration_ms(&mut self, ms: f64);
    /// Record the finalized body bytes (`response_bytes` or `bytes`).
    fn set_bytes(&mut self, bytes: u64);
}

/// Shared per-request call chain. The worker thread appends; the request
/// handler reads it even when a timeout orphans the worker. Lock poisoning
/// recovers so evidence survives a panicked holder instead of hiding a bug.
#[derive(Debug)]
pub struct Chain<T> {
    inner: Arc<Mutex<Vec<T>>>,
}

impl<T> Chain<T> {
    /// Empty chain for one request.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Append one call and return its slot.
    pub fn begin(&self, record: T) -> usize {
        let mut calls = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.push(record);
        calls.len().saturating_sub(1)
    }

    /// Update one call in place; an out-of-range index is a no-op.
    pub fn update(&self, index: usize, update: impl FnOnce(&mut T)) {
        let mut calls = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = calls.get_mut(index) {
            update(record);
        }
    }

    /// Snapshot the chain for the structured log.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Json>
    where
        T: LogRecord,
    {
        let calls = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.iter().map(<T as LogRecord>::to_json).collect()
    }
}

impl<T> Clone for Chain<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Default for Chain<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle that finalizes one streamed call when its body ends. Dropping it
/// without an explicit finish records an abandoned stream with zero bytes, so
/// the call chain never keeps a half-open entry when a script discards a
/// streamed response.
#[derive(Debug)]
pub struct Pending<T> {
    chain: Chain<T>,
    index: usize,
    started: Instant,
    finished: AtomicBool,
    set_duration: fn(&mut T, f64),
    set_bytes: fn(&mut T, u64),
}

impl<T> Pending<T> {
    /// Watch one begun call until its stream ends.
    #[must_use]
    pub fn new(chain: Chain<T>, index: usize, started: Instant) -> Self
    where
        T: LogRecord,
    {
        Self {
            chain,
            index,
            started,
            finished: AtomicBool::new(false),
            set_duration: <T as LogRecord>::set_duration_ms,
            set_bytes: <T as LogRecord>::set_bytes,
        }
    }

    /// Finalize this call with its streamed bytes plus a caller-supplied
    /// error mapping.
    pub fn finish_with(&self, bytes: u64, update: impl FnOnce(&mut T)) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let ms = elapsed_ms(self.started.elapsed());
        let set_duration = self.set_duration;
        let set_bytes = self.set_bytes;
        self.chain.update(self.index, |record| {
            set_duration(record, ms);
            set_bytes(record, bytes);
            update(record);
        });
    }

    /// Snapshot the chain after this stream finalized it.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Json>
    where
        T: LogRecord,
    {
        self.chain.snapshot()
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let ms = elapsed_ms(self.started.elapsed());
        let set_duration = self.set_duration;
        let set_bytes = self.set_bytes;
        self.chain.update(self.index, |record| {
            set_duration(record, ms);
            set_bytes(record, 0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Record {
        duration_ms: Option<f64>,
        bytes: Option<u64>,
        error: Option<&'static str>,
    }

    impl LogRecord for Record {
        fn to_json(&self) -> Json {
            serde_json::json!({
                "duration_ms": self.duration_ms,
                "bytes": self.bytes,
                "error": self.error,
            })
        }

        fn set_duration_ms(&mut self, ms: f64) {
            self.duration_ms = Some(ms);
        }

        fn set_bytes(&mut self, bytes: u64) {
            self.bytes = Some(bytes);
        }
    }

    fn record() -> Record {
        Record {
            duration_ms: None,
            bytes: None,
            error: None,
        }
    }

    #[test]
    fn chain_records_and_snapshots_in_order() {
        let chain: Chain<Record> = Chain::new();
        let first = chain.begin(record());
        let second = chain.begin(record());
        assert_eq!((first, second), (0, 1));
        chain.update(first, |record| record.error = Some("boom"));
        let snapshot = chain.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0]["error"], "boom");
        assert_eq!(snapshot[1]["error"], serde_json::Value::Null);
    }

    #[test]
    fn pending_finish_writes_duration_bytes_and_error() {
        let chain: Chain<Record> = Chain::new();
        let index = chain.begin(record());
        let pending = Pending::new(chain.clone(), index, Instant::now());
        pending.finish_with(13, |record| record.error = Some("upstream_stream_error"));
        let snapshot = chain.snapshot();
        assert_eq!(snapshot[0]["bytes"], 13);
        assert_eq!(snapshot[0]["error"], "upstream_stream_error");
        assert!(
            snapshot[0]["duration_ms"].is_number(),
            "snapshot: {snapshot:?}"
        );
        // A second finish is a no-op; the first result stands.
        pending.finish_with(99, |record| record.error = Some("other"));
        let snapshot = chain.snapshot();
        assert_eq!(snapshot[0]["bytes"], 13);
    }

    #[test]
    fn pending_drop_finalizes_an_abandoned_stream() {
        let chain: Chain<Record> = Chain::new();
        let index = chain.begin(record());
        drop(Pending::new(chain.clone(), index, Instant::now()));
        let snapshot = chain.snapshot();
        assert_eq!(snapshot[0]["bytes"], 0);
        assert!(
            snapshot[0]["duration_ms"].is_number(),
            "snapshot: {snapshot:?}"
        );
    }

    #[test]
    fn out_of_range_updates_are_noops() {
        let chain: Chain<Record> = Chain::new();
        chain.update(usize::MAX, |_| panic!("must not run"));
        assert!(chain.snapshot().is_empty());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn elapsed_ms_rounds_to_two_decimals() {
        assert_eq!(elapsed_ms(Duration::from_millis(13)), 13.0);
        assert_eq!(elapsed_ms(Duration::from_micros(1_234)), 1.23);
    }
}
