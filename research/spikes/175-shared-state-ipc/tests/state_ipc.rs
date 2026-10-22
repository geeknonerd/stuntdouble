use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use shared_state_spike::{Host, WorkerError};

fn worker_executable() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_state-spike")
        .map(PathBuf::from)
        .expect("Cargo must provide the spike worker executable")
}

#[test]
fn worker_round_trips_all_state_operations() {
    let host = Host::new(worker_executable());
    let response = host
        .run_script(
            r#"
ctx.state.set("demo:key", { nested: [1, true, "x"] });
var first = ctx.state.get("demo:key");
first.nested.push(99);
var second = ctx.state.get("demo:key");
var setIfAbsent = ctx.state.setIfAbsent("demo:key", "other");
var taken = ctx.state.take("demo:key");
var missing = ctx.state.get("demo:key");
ctx.respond({
  first: first,
  second: second,
  setIfAbsent: setIfAbsent,
  taken: taken,
  missing: missing === undefined
});
"#,
            Duration::from_secs(5),
        )
        .expect("worker script should succeed");

    assert_eq!(
        response,
        json!({
            "first": { "nested": [1, true, "x", 99] },
            "second": { "nested": [1, true, "x"] },
            "setIfAbsent": false,
            "taken": { "nested": [1, true, "x"] },
            "missing": true
        })
    );
}

#[test]
fn concurrent_take_has_exactly_one_winner() {
    let host = Arc::new(Host::new(worker_executable()));
    for iteration in 0..20 {
        let key = format!("take:{iteration}");
        host.run_script(
            &format!(r#"ctx.state.set("{key}", "grant"); ctx.respond(true);"#),
            Duration::from_secs(5),
        )
        .expect("seed state");

        let barrier = Arc::new(Barrier::new(2));
        let attempts = (0..2)
            .map(|_| {
                let host = Arc::clone(&host);
                let barrier = Arc::clone(&barrier);
                let script =
                    format!(r#"ctx.respond({{ won: ctx.state.take("{key}") !== undefined }});"#);
                thread::spawn(move || {
                    host.run_script_with_gate(&script, Duration::from_secs(5), &barrier)
                        .expect("take worker")
                })
            })
            .collect::<Vec<_>>();
        let responses = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("join take worker"))
            .collect::<Vec<_>>();
        let winners = responses
            .iter()
            .filter(|response| {
                response.get("won").and_then(serde_json::Value::as_bool) == Some(true)
            })
            .count();
        assert_eq!(winners, 1, "iteration {iteration}: {responses:?}");
    }
}

#[test]
fn concurrent_set_if_absent_has_exactly_one_winner() {
    let host = Arc::new(Host::new(worker_executable()));
    for iteration in 0..20 {
        let key = format!("request:{iteration}");
        let barrier = Arc::new(Barrier::new(2));
        let attempts = ["left", "right"]
            .into_iter()
            .map(|value| {
                let host = Arc::clone(&host);
                let barrier = Arc::clone(&barrier);
                let script = format!(
                    r#"ctx.respond({{ stored: ctx.state.setIfAbsent("{key}", "{value}"), value: "{value}" }});"#
                );
                thread::spawn(move || {
                    host.run_script_with_gate(&script, Duration::from_secs(5), &barrier)
                        .expect("setIfAbsent worker")
                })
            })
            .collect::<Vec<_>>();
        let responses = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("join setIfAbsent worker"))
            .collect::<Vec<_>>();
        let winners = responses
            .iter()
            .filter(|response| response["stored"].as_bool() == Some(true))
            .count();
        assert_eq!(winners, 1, "iteration {iteration}: {responses:?}");

        let final_value = host
            .run_script(
                &format!(r#"ctx.respond(ctx.state.get("{key}"));"#),
                Duration::from_secs(5),
            )
            .expect("read winner");
        let winning_value = responses
            .iter()
            .find(|response| response["stored"].as_bool() == Some(true))
            .expect("one winning response")["value"]
            .clone();
        assert_eq!(final_value, winning_value);
    }
}

#[test]
fn timeout_is_reported_as_a_worker_error() {
    let host = Host::new(worker_executable());
    let error = host
        .run_script("while (true) {}", Duration::from_millis(50))
        .expect_err("the worker must time out");
    assert!(matches!(error, WorkerError::Timeout), "{error:?}");
}

#[test]
fn ttl_set_through_one_worker_is_visible_to_another_worker() {
    let host = Host::new(worker_executable());
    host.run_script(
        r#"ctx.state.set("ttl:key", "value", { ttl_ms: 3600000 }); ctx.respond(true);"#,
        Duration::from_secs(5),
    )
    .expect("set ttl");

    let fresh = host
        .run_script(
            r#"ctx.respond({ found: ctx.state.get("ttl:key") !== undefined });"#,
            Duration::from_secs(5),
        )
        .expect("fresh read");
    assert_eq!(fresh, json!({ "found": true }));

    thread::sleep(Duration::from_millis(100));
    let before_boundary = host
        .run_script(
            r#"ctx.respond({ found: ctx.state.get("ttl:key") !== undefined });"#,
            Duration::from_secs(5),
        )
        .expect("second read");
    assert_eq!(before_boundary, json!({ "found": true }));
}

#[test]
fn default_entry_capacity_fails_closed_without_eviction() {
    let host = Host::new(worker_executable());
    let response = host
        .run_script(
            r#"
var stored = 0;
var code = null;
try {
  for (var i = 0; i < 1025; i++) {
    ctx.state.set("capacity:" + i, i);
    stored += 1;
  }
} catch (error) {
  code = error.code;
}
ctx.respond({
  stored: stored,
  code: code,
  first: ctx.state.get("capacity:0"),
  last: ctx.state.get("capacity:1023"),
  extra: ctx.state.get("capacity:1024") === undefined
});
"#,
            Duration::from_secs(30),
        )
        .expect("capacity scenario");
    assert_eq!(
        response,
        json!({
            "stored": 1024,
            "code": "state_capacity_exceeded",
            "first": 0,
            "last": 1023,
            "extra": true
        })
    );
}

#[test]
fn restart_clears_host_state_but_separate_workers_still_share_it() {
    let worker = worker_executable();
    let first_host = Host::new(worker.clone());
    first_host
        .run_script(
            r#"
globalThis.__sd_leak = "worker-global";
ctx.state.set("restart:key", "host-state");
ctx.respond(true);
"#,
            Duration::from_secs(5),
        )
        .expect("seed first host");
    let same_host = first_host
        .run_script(
            r#"
ctx.respond({
  globalLeaked: typeof globalThis.__sd_leak !== "undefined",
  state: ctx.state.get("restart:key")
});
"#,
            Duration::from_secs(5),
        )
        .expect("read from a fresh worker");
    assert_eq!(
        same_host,
        json!({ "globalLeaked": false, "state": "host-state" })
    );

    let restarted_host = Host::new(worker);
    let after_restart = restarted_host
        .run_script(
            r#"ctx.respond(ctx.state.get("restart:key") === undefined);"#,
            Duration::from_secs(5),
        )
        .expect("read after restart");
    assert_eq!(after_restart, json!(true));
}

#[test]
fn state_writes_survive_script_errors_and_worker_timeouts() {
    let host = Host::new(worker_executable());
    let error = host
        .run_script(
            r#"ctx.state.set("commit:error", "kept"); throw new Error("boom");"#,
            Duration::from_secs(5),
        )
        .expect_err("script must fail");
    assert!(matches!(error, WorkerError::Script(_)), "{error:?}");
    assert_eq!(
        host.run_script(
            r#"ctx.respond(ctx.state.get("commit:error"));"#,
            Duration::from_secs(5)
        )
        .expect("read after script error"),
        json!("kept")
    );

    let started = Instant::now();
    let error = host
        .run_script(
            r#"ctx.state.set("commit:timeout", "kept"); while (true) {}"#,
            Duration::from_secs(2),
        )
        .expect_err("worker must time out");
    assert!(matches!(error, WorkerError::Timeout), "{error:?}");
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(
        host.run_script(
            r#"ctx.respond(ctx.state.get("commit:timeout"));"#,
            Duration::from_secs(5)
        )
        .expect("read after worker timeout"),
        json!("kept")
    );
}

#[test]
fn worker_side_validation_rejects_lossy_values_before_json_serialization() {
    let host = Host::new(worker_executable());
    let response = host
        .run_script(
            r#"
var codes = [];
function record(run) {
  try { run(); codes.push("accepted"); }
  catch (error) { codes.push(error.code); }
}
record(function () { ctx.state.set("invalid:undefined", undefined); });
record(function () { ctx.state.set("invalid:function", function () {}); });
record(function () { ctx.state.set("invalid:nan", NaN); });
record(function () { ctx.state.set("invalid:sparse", [1, , 3]); });
record(function () { ctx.state.set("invalid:date", new Date(0)); });
record(function () { var cycle = {}; cycle.self = cycle; ctx.state.set("invalid:cycle", cycle); });
record(function () { ctx.state.set("invalid:undefined-property", { value: undefined }); });
record(function () { ctx.state.set("invalid:surrogate", "\uD800"); });
record(function () { ctx.state.set("\uDC00", "no"); });
record(function () { var array = [1]; array[4294967295] = 2; ctx.state.set("invalid:array-index", array); });
record(function () { var array = [1]; array[Symbol("extra")] = 2; ctx.state.set("invalid:array-symbol", array); });
record(function () { var array = [1]; array["01"] = 2; ctx.state.set("invalid:array-leading-zero", array); });
record(function () { var array = [1]; array["1e0"] = 2; ctx.state.set("invalid:array-exponent", array); });
record(function () { var array = [1]; array[""] = 2; ctx.state.set("invalid:array-empty-key", array); });
ctx.respond({
  codes: codes,
  stored: ctx.state.get("invalid:undefined") === undefined
});
"#,
            Duration::from_secs(5),
        )
        .expect("validation scenario");
    assert_eq!(
        response,
        json!({
            "codes": [
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input",
                "state_invalid_input"
            ],
            "stored": true
        })
    );
}

#[test]
fn key_and_value_limits_are_enforced_at_the_boundary() {
    let host = Host::new(worker_executable());
    let response = host
        .run_script(
            r#"
var maxKey = "k".repeat(256);
ctx.state.set(maxKey, "ok");
var keyCode = null;
try { ctx.state.set("k".repeat(257), "no"); }
catch (error) { keyCode = error.code; }

var maxValue = "x".repeat(16382);
ctx.state.set("max-value", maxValue);
var valueCode = null;
try { ctx.state.set("over-value", "x".repeat(16383)); }
catch (error) { valueCode = error.code; }

var ttlCode = null;
try { ctx.state.set("bad-ttl", "no", { ttl_ms: 0 }); }
catch (error) { ttlCode = error.code; }

ctx.respond({
  maxKey: ctx.state.get(maxKey),
  keyCode: keyCode,
  maxValueLength: ctx.state.get("max-value").length,
  valueCode: valueCode,
  ttlCode: ttlCode
});
"#,
            Duration::from_secs(10),
        )
        .expect("limits scenario");
    assert_eq!(
        response,
        json!({
            "maxKey": "ok",
            "keyCode": "state_invalid_input",
            "maxValueLength": 16382,
            "valueCode": "state_invalid_input",
            "ttlCode": "state_invalid_input"
        })
    );
}
