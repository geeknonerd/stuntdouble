use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use shared_state_spike::{
    ManualClock, StateError, Store, StoreConfig, MAX_ENTRIES, MAX_KEY_BYTES, MAX_TOTAL_BYTES,
    MAX_TTL_MS, MAX_VALUE_BYTES,
};

fn store_with(clock: &Arc<ManualClock>, max_entries: usize, max_total_bytes: usize) -> Store {
    Store::new(
        StoreConfig {
            max_entries,
            max_total_bytes,
            ..StoreConfig::default()
        },
        Arc::clone(clock) as Arc<dyn shared_state_spike::Clock>,
    )
}

#[test]
fn default_limits_match_the_adr() {
    let config = StoreConfig::default();
    assert_eq!(config.max_entries, MAX_ENTRIES);
    assert_eq!(config.max_key_bytes, MAX_KEY_BYTES);
    assert_eq!(config.max_total_bytes, MAX_TOTAL_BYTES);
    assert_eq!(config.max_value_bytes, MAX_VALUE_BYTES);
    assert_eq!(config.max_ttl, Duration::from_millis(MAX_TTL_MS));
}

#[test]
fn ttl_expires_at_the_exact_boundary_and_reads_do_not_slide_it() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 8, 1024);
    store
        .set("key", json!("value"), Some(Duration::from_millis(100)))
        .expect("set ttl");

    clock.advance(Duration::from_millis(60));
    assert_eq!(store.get("key").expect("first read"), Some(json!("value")));

    clock.advance(Duration::from_millis(39));
    assert_eq!(
        store.get("key").expect("read before boundary"),
        Some(json!("value"))
    );

    clock.advance(Duration::from_millis(1));
    assert_eq!(store.get("key").expect("read at boundary"), None);
}

#[test]
fn entry_capacity_fails_closed_without_eviction() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 2, 1024);
    store.set("a", json!(1), None).expect("set a");
    store.set("b", json!(2), None).expect("set b");

    assert_eq!(
        store.set("c", json!(3), None),
        Err(StateError::CapacityExceeded)
    );
    assert_eq!(store.get("a").expect("get a"), Some(json!(1)));
    assert_eq!(store.get("b").expect("get b"), Some(json!(2)));
    assert_eq!(store.get("c").expect("get c"), None);
}

#[test]
fn byte_capacity_and_failed_replacement_preserve_the_old_value_and_ttl() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 8, 12);
    store
        .set("a", json!("old"), Some(Duration::from_millis(100)))
        .expect("set old value");
    store.set("b", json!("123"), None).expect("set b");

    assert_eq!(
        store.set("a", json!("too-large"), None),
        Err(StateError::CapacityExceeded)
    );
    clock.advance(Duration::from_millis(99));
    assert_eq!(store.get("a").expect("old value"), Some(json!("old")));
    clock.advance(Duration::from_millis(1));
    assert_eq!(store.get("a").expect("old ttl boundary"), None);
}

#[test]
fn expired_entries_release_capacity() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 1, 1024);
    store
        .set("a", json!(1), Some(Duration::from_millis(10)))
        .expect("set expiring");
    clock.advance(Duration::from_millis(10));
    store.set("b", json!(2), None).expect("reuse capacity");
    assert_eq!(store.get("b").expect("get b"), Some(json!(2)));
}

#[test]
fn set_if_absent_replaces_an_expired_key_at_entry_capacity() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 1, 1024);
    store
        .set("key", json!("old"), Some(Duration::from_millis(10)))
        .expect("set expiring");
    clock.advance(Duration::from_millis(10));
    assert!(store
        .set_if_absent("key", json!("new"), None)
        .expect("replace expired"));
    assert_eq!(store.get("key").expect("new value"), Some(json!("new")));
}

#[test]
fn set_replaces_the_value_and_resets_the_ttl_origin() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 8, 1024);
    store
        .set("key", json!("old"), Some(Duration::from_millis(100)))
        .expect("set old");
    clock.advance(Duration::from_millis(60));
    store
        .set("key", json!("new"), Some(Duration::from_millis(100)))
        .expect("replace");
    clock.advance(Duration::from_millis(60));
    assert_eq!(store.get("key").expect("new ttl"), Some(json!("new")));
    clock.advance(Duration::from_millis(40));
    assert_eq!(store.get("key").expect("new boundary"), None);
}

#[test]
fn default_total_byte_capacity_fails_closed() {
    let clock = Arc::new(ManualClock::new(Instant::now()));
    let mut store = store_with(&clock, 1024, 8 * 1024 * 1024);
    let value = json!("x".repeat(MAX_VALUE_BYTES - 2));

    for index in 0..511 {
        store
            .set(&format!("k{index:03}"), value.clone(), None)
            .expect("fill byte capacity");
    }
    assert_eq!(
        store.set("k511", value, None),
        Err(StateError::CapacityExceeded)
    );
    assert!(store.get("k000").expect("first entry").is_some());
    assert!(store.get("k510").expect("last accepted entry").is_some());
    assert_eq!(store.get("k511").expect("rejected entry"), None);
}
