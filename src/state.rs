//! Process-local Shared state owned by one running `serve` instance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

pub(crate) const MAX_KEY_BYTES: usize = 256;
pub(crate) const MAX_VALUE_BYTES: usize = 16 * 1024;
pub(crate) const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 1024;
pub(crate) const MAX_TTL_MS: u64 = 604_800_000;
pub(crate) const MAX_VALUE_DEPTH: usize = 32;

/// One state store shared by every script worker in a single server instance.
pub(crate) type SharedState = Arc<Mutex<Store>>;

/// Create the empty state store for one server instance.
pub(crate) fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(Store::new(Arc::new(RealClock))))
}

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateError {
    InvalidInput,
    CapacityExceeded,
    Internal,
}

impl StateError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "state_invalid_input",
            Self::CapacityExceeded => "state_capacity_exceeded",
            Self::Internal => "state_internal_error",
        }
    }
}

pub(crate) struct Store {
    clock: Arc<dyn Clock>,
    entries: HashMap<String, Entry>,
    used_bytes: usize,
}

#[derive(Debug)]
struct Entry {
    value: Json,
    value_bytes: usize,
    expires_at: Option<Instant>,
}

impl Store {
    fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            entries: HashMap::new(),
            used_bytes: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &str) -> Result<Option<Json>, StateError> {
        self.validate_key(key)?;
        let now = self.clock.now();
        if self.remove_if_expired(key, now)? {
            return Ok(None);
        }
        Ok(self.entries.get(key).map(|entry| entry.value.clone()))
    }

    pub(crate) fn set(
        &mut self,
        key: &str,
        value: Json,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        self.validate_key(key)?;
        let value_bytes = self.validate_value(&value)?;
        self.validate_ttl(ttl)?;
        let now = self.clock.now();
        self.remove_all_expired(now)?;
        self.commit_write(key, value, value_bytes, ttl, now)
    }

    pub(crate) fn take(&mut self, key: &str) -> Result<Option<Json>, StateError> {
        self.validate_key(key)?;
        let now = self.clock.now();
        if self.remove_if_expired(key, now)? {
            return Ok(None);
        }
        Ok(self.remove_entry(key)?.map(|entry| entry.value))
    }

    pub(crate) fn set_if_absent(
        &mut self,
        key: &str,
        value: Json,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        self.validate_key(key)?;
        let value_bytes = self.validate_value(&value)?;
        self.validate_ttl(ttl)?;
        let now = self.clock.now();
        self.remove_all_expired(now)?;
        if self.entries.contains_key(key) {
            return Ok(false);
        }
        self.commit_write(key, value, value_bytes, ttl, now)?;
        Ok(true)
    }

    fn commit_write(
        &mut self,
        key: &str,
        value: Json,
        value_bytes: usize,
        ttl: Option<Duration>,
        now: Instant,
    ) -> Result<(), StateError> {
        let new_cost = entry_cost(key, value_bytes).ok_or(StateError::CapacityExceeded)?;
        let replacing = self.entries.contains_key(key);
        if !replacing && self.entries.len() >= MAX_ENTRIES {
            return Err(StateError::CapacityExceeded);
        }
        let old_cost = self.entries.get(key).map_or(Ok(0), |entry| {
            entry_cost(key, entry.value_bytes).ok_or(StateError::Internal)
        })?;
        let after_removal = self
            .used_bytes
            .checked_sub(old_cost)
            .ok_or(StateError::Internal)?;
        let new_used = after_removal
            .checked_add(new_cost)
            .ok_or(StateError::CapacityExceeded)?;
        if new_used > MAX_TOTAL_BYTES {
            return Err(StateError::CapacityExceeded);
        }
        let expires_at = self.expires_at(now, ttl)?;
        self.entries.insert(
            key.to_string(),
            Entry {
                value,
                value_bytes,
                expires_at,
            },
        );
        self.used_bytes = new_used;
        Ok(())
    }

    fn expires_at(
        &self,
        now: Instant,
        ttl: Option<Duration>,
    ) -> Result<Option<Instant>, StateError> {
        ttl.map(|ttl| now.checked_add(ttl).ok_or(StateError::Internal))
            .transpose()
    }

    fn validate_key(&self, key: &str) -> Result<(), StateError> {
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return Err(StateError::InvalidInput);
        }
        Ok(())
    }

    fn validate_value(&self, value: &Json) -> Result<usize, StateError> {
        validate_json_value(value, 0)?;
        let compact = serde_json::to_vec(value).map_err(|_| StateError::InvalidInput)?;
        if compact.len() > MAX_VALUE_BYTES {
            return Err(StateError::InvalidInput);
        }
        Ok(compact.len())
    }

    fn validate_ttl(&self, ttl: Option<Duration>) -> Result<(), StateError> {
        if ttl.is_some_and(|ttl| ttl.is_zero() || ttl > Duration::from_millis(MAX_TTL_MS)) {
            return Err(StateError::InvalidInput);
        }
        Ok(())
    }

    fn remove_if_expired(&mut self, key: &str, now: Instant) -> Result<bool, StateError> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|entry| entry.is_expired(now));
        if expired {
            self.remove_entry(key)?.ok_or(StateError::Internal)?;
        }
        Ok(expired)
    }

    fn remove_all_expired(&mut self, now: Instant) -> Result<(), StateError> {
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.is_expired(now))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            self.remove_entry(&key)?.ok_or(StateError::Internal)?;
        }
        Ok(())
    }

    fn remove_entry(&mut self, key: &str) -> Result<Option<Entry>, StateError> {
        let Some(entry) = self.entries.get(key) else {
            return Ok(None);
        };
        let cost = entry_cost(key, entry.value_bytes).ok_or(StateError::Internal)?;
        let new_used = self
            .used_bytes
            .checked_sub(cost)
            .ok_or(StateError::Internal)?;
        let entry = self.entries.remove(key).ok_or(StateError::Internal)?;
        self.used_bytes = new_used;
        Ok(Some(entry))
    }
}

fn entry_cost(key: &str, value_bytes: usize) -> Option<usize> {
    key.len().checked_add(value_bytes)
}

fn validate_json_value(value: &Json, depth: usize) -> Result<(), StateError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(StateError::InvalidInput);
    }
    match value {
        Json::Array(items) => {
            for item in items {
                validate_json_value(item, depth + 1)?;
            }
        }
        Json::Object(entries) => {
            for value in entries.values() {
                validate_json_value(value, depth + 1)?;
            }
        }
        Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => {}
    }
    Ok(())
}

impl Entry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct ManualClock {
        now: Mutex<Instant>,
    }

    impl ManualClock {
        fn new(now: Instant) -> Self {
            Self {
                now: Mutex::new(now),
            }
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().expect("manual clock");
            *now += duration;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            *self.now.lock().expect("manual clock")
        }
    }

    #[test]
    fn ttl_expires_at_the_exact_boundary_and_reads_do_not_slide_it() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set(
                "key",
                serde_json::json!("value"),
                Some(Duration::from_millis(100)),
            )
            .expect("set ttl");

        clock.advance(Duration::from_millis(60));
        assert_eq!(
            store.get("key").expect("read before boundary"),
            Some(serde_json::json!("value"))
        );

        clock.advance(Duration::from_millis(39));
        assert_eq!(
            store.get("key").expect("read near boundary"),
            Some(serde_json::json!("value"))
        );

        clock.advance(Duration::from_millis(1));
        assert_eq!(store.get("key").expect("read at boundary"), None);
    }

    #[test]
    fn set_replaces_the_value_and_resets_the_ttl_origin() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set(
                "key",
                serde_json::json!("old"),
                Some(Duration::from_millis(100)),
            )
            .expect("set old value");

        clock.advance(Duration::from_millis(60));
        store
            .set(
                "key",
                serde_json::json!("new"),
                Some(Duration::from_millis(100)),
            )
            .expect("replace value");
        clock.advance(Duration::from_millis(60));
        assert_eq!(
            store.get("key").expect("new ttl is still live"),
            Some(serde_json::json!("new"))
        );
        clock.advance(Duration::from_millis(40));
        assert_eq!(store.get("key").expect("new ttl boundary"), None);
    }

    #[test]
    fn byte_capacity_and_failed_replacement_preserve_the_old_value_and_ttl() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set(
                "seed",
                serde_json::json!("old"),
                Some(Duration::from_millis(100)),
            )
            .expect("seed expiring value");
        let full_value = serde_json::json!("x".repeat(MAX_VALUE_BYTES - 2));
        for index in 0..511 {
            store
                .set(&format!("k{index:03}"), full_value.clone(), None)
                .expect("fill total byte capacity");
        }

        assert_eq!(
            store.set("seed", full_value, None),
            Err(StateError::CapacityExceeded)
        );
        clock.advance(Duration::from_millis(99));
        assert_eq!(
            store.get("seed").expect("old value remains"),
            Some(serde_json::json!("old"))
        );
        clock.advance(Duration::from_millis(1));
        assert_eq!(store.get("seed").expect("old ttl remains"), None);
    }

    #[test]
    fn expired_entries_release_entry_capacity_before_a_write() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        for index in 0..MAX_ENTRIES {
            store
                .set(
                    &format!("k{index}"),
                    serde_json::json!(index),
                    Some(Duration::from_millis(10)),
                )
                .expect("fill entry capacity");
        }
        clock.advance(Duration::from_millis(10));
        store
            .set("replacement", serde_json::json!("value"), None)
            .expect("expired entries release capacity");
        assert_eq!(
            store.get("replacement").expect("new value"),
            Some(serde_json::json!("value"))
        );
    }

    #[test]
    fn take_releases_the_accounted_capacity() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set("key", serde_json::json!("value"), None)
            .expect("set value");
        assert!(store.used_bytes > 0);

        assert_eq!(
            store.take("key").expect("take value"),
            Some(serde_json::json!("value"))
        );
        assert_eq!(store.used_bytes, 0);
    }

    #[test]
    fn set_if_absent_treats_an_expired_key_as_absent() {
        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set(
                "key",
                serde_json::json!("old"),
                Some(Duration::from_millis(10)),
            )
            .expect("set expiring value");
        clock.advance(Duration::from_millis(10));

        assert!(store
            .set_if_absent("key", serde_json::json!("new"), None)
            .expect("replace expired key"));
        assert_eq!(
            store.get("key").expect("new value"),
            Some(serde_json::json!("new"))
        );
    }

    #[test]
    fn host_enforces_the_depth_limit_and_preserves_the_previous_value() {
        fn nested_object(depth: usize) -> Json {
            let mut value = serde_json::json!({});
            for _ in 0..depth {
                value = serde_json::json!({ "next": value });
            }
            value
        }

        let clock = Arc::new(ManualClock::new(Instant::now()));
        let mut store = Store::new(Arc::clone(&clock) as Arc<dyn Clock>);
        store
            .set("key", serde_json::json!("old"), None)
            .expect("seed value");

        for depth in [33, 40] {
            assert_eq!(
                store.set("key", nested_object(depth), None),
                Err(StateError::InvalidInput),
                "depth {depth} must fail"
            );
            assert_eq!(
                store.get("key").expect("read preserved value"),
                Some(serde_json::json!("old"))
            );
        }

        store
            .set("key", nested_object(MAX_VALUE_DEPTH), None)
            .expect("maximum supported depth");
    }
}
