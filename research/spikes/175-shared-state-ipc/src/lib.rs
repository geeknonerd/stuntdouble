use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use boa_engine::native_function::NativeFunction;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsString, JsValue, Source};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

pub const MAX_KEY_BYTES: usize = 256;
pub const MAX_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 1024;
pub const MAX_TTL_MS: u64 = 604_800_000;

const MAX_PROTOCOL_LINE_BYTES: u64 = 64 * 1024;

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Instant>,
}

impl ManualClock {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().expect("manual clock lock");
        *now += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.now.lock().expect("manual clock lock")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StoreConfig {
    pub max_entries: usize,
    pub max_total_bytes: usize,
    pub max_value_bytes: usize,
    pub max_key_bytes: usize,
    pub max_ttl: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_value_bytes: MAX_VALUE_BYTES,
            max_key_bytes: MAX_KEY_BYTES,
            max_ttl: Duration::from_millis(MAX_TTL_MS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    InvalidInput,
    CapacityExceeded,
    Internal,
}

impl StateError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "state_invalid_input",
            Self::CapacityExceeded => "state_capacity_exceeded",
            Self::Internal => "state_internal_error",
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for StateError {}

struct Entry {
    value: Json,
    value_bytes: usize,
    expires_at: Option<Instant>,
}

pub struct Store {
    config: StoreConfig,
    clock: Arc<dyn Clock>,
    entries: HashMap<String, Entry>,
    used_bytes: usize,
}

impl Store {
    #[must_use]
    pub fn new(config: StoreConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            config,
            clock,
            entries: HashMap::new(),
            used_bytes: 0,
        }
    }

    pub fn get(&mut self, key: &str) -> Result<Option<Json>, StateError> {
        self.validate_key(key)?;
        let now = self.clock.now();
        if self.remove_if_expired(key, now) {
            return Ok(None);
        }
        Ok(self.entries.get(key).map(|entry| entry.value.clone()))
    }

    pub fn set(&mut self, key: &str, value: Json, ttl: Option<Duration>) -> Result<(), StateError> {
        self.validate_key(key)?;
        let value_bytes = self.validate_value(&value)?;
        self.validate_ttl(ttl)?;
        self.remove_all_expired();

        let key_bytes = key.len();
        let new_cost = key_bytes
            .checked_add(value_bytes)
            .ok_or(StateError::CapacityExceeded)?;
        if let Some(existing) = self.entries.get(key) {
            let old_cost = key_bytes
                .checked_add(existing.value_bytes)
                .ok_or(StateError::Internal)?;
            let new_used = self
                .used_bytes
                .checked_sub(old_cost)
                .and_then(|used| used.checked_add(new_cost))
                .ok_or(StateError::Internal)?;
            if new_used > self.config.max_total_bytes {
                return Err(StateError::CapacityExceeded);
            }
            self.used_bytes = new_used;
            self.entries.insert(
                key.to_string(),
                Entry {
                    value,
                    value_bytes,
                    expires_at: self.expiry(ttl),
                },
            );
            return Ok(());
        }

        if self.entries.len() >= self.config.max_entries
            || self
                .used_bytes
                .checked_add(new_cost)
                .is_none_or(|used| used > self.config.max_total_bytes)
        {
            return Err(StateError::CapacityExceeded);
        }
        self.insert_new_entry(key, value, value_bytes, ttl, new_cost);
        Ok(())
    }

    pub fn take(&mut self, key: &str) -> Result<Option<Json>, StateError> {
        self.validate_key(key)?;
        let now = self.clock.now();
        if self.remove_if_expired(key, now) {
            return Ok(None);
        }
        let Some(entry) = self.entries.remove(key) else {
            return Ok(None);
        };
        self.used_bytes = self
            .used_bytes
            .checked_sub(key.len() + entry.value_bytes)
            .ok_or(StateError::Internal)?;
        Ok(Some(entry.value))
    }

    pub fn set_if_absent(
        &mut self,
        key: &str,
        value: Json,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        self.validate_key(key)?;
        let value_bytes = self.validate_value(&value)?;
        self.validate_ttl(ttl)?;
        self.remove_all_expired();
        let now = self.clock.now();
        if self
            .entries
            .get(key)
            .is_some_and(|entry| !entry.is_expired(now))
        {
            return Ok(false);
        }

        let key_bytes = key.len();
        let new_cost = key_bytes
            .checked_add(value_bytes)
            .ok_or(StateError::CapacityExceeded)?;
        if self.entries.len() >= self.config.max_entries
            || self
                .used_bytes
                .checked_add(new_cost)
                .is_none_or(|used| used > self.config.max_total_bytes)
        {
            return Err(StateError::CapacityExceeded);
        }
        self.insert_new_entry(key, value, value_bytes, ttl, new_cost);
        Ok(true)
    }

    fn validate_key(&self, key: &str) -> Result<(), StateError> {
        if key.is_empty() || key.len() > self.config.max_key_bytes {
            return Err(StateError::InvalidInput);
        }
        Ok(())
    }

    fn validate_value(&self, value: &Json) -> Result<usize, StateError> {
        let compact = serde_json::to_vec(value).map_err(|_| StateError::InvalidInput)?;
        if compact.len() > self.config.max_value_bytes {
            return Err(StateError::InvalidInput);
        }
        Ok(compact.len())
    }

    fn validate_ttl(&self, ttl: Option<Duration>) -> Result<(), StateError> {
        if ttl.is_some_and(|ttl| ttl.is_zero() || ttl > self.config.max_ttl) {
            return Err(StateError::InvalidInput);
        }
        Ok(())
    }

    fn expiry(&self, ttl: Option<Duration>) -> Option<Instant> {
        ttl.and_then(|ttl| self.clock.now().checked_add(ttl))
    }

    fn insert_new_entry(
        &mut self,
        key: &str,
        value: Json,
        value_bytes: usize,
        ttl: Option<Duration>,
        new_cost: usize,
    ) {
        self.used_bytes += new_cost;
        self.entries.insert(
            key.to_string(),
            Entry {
                value,
                value_bytes,
                expires_at: self.expiry(ttl),
            },
        );
    }

    fn remove_if_expired(&mut self, key: &str, now: Instant) -> bool {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|entry| entry.is_expired(now));
        if expired {
            self.remove_entry(key);
        }
        expired
    }

    fn remove_all_expired(&mut self) {
        let now = self.clock.now();
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.is_expired(now))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            self.remove_entry(&key);
        }
    }

    fn remove_entry(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.used_bytes = self
                .used_bytes
                .saturating_sub(key.len() + entry.value_bytes);
        }
    }
}

impl Entry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum BridgeName {
    #[serde(rename = "__sd_state")]
    State,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ParentMessage {
    Job { script: String, request: Json },
    HostResult { result: Json },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerMessage {
    HostCall {
        name: BridgeName,
        payload: Json,
    },
    FinalResult {
        ok: bool,
        #[serde(flatten)]
        body: FinalBody,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FinalBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<Json>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    Timeout,
    Terminated,
    Protocol(String),
    Script(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("worker timed out"),
            Self::Terminated => formatter.write_str("worker terminated"),
            Self::Protocol(message) => write!(formatter, "protocol error: {message}"),
            Self::Script(message) => write!(formatter, "script error: {message}"),
        }
    }
}

impl std::error::Error for WorkerError {}

pub struct Host {
    worker_executable: PathBuf,
    store: Arc<Mutex<Store>>,
}

impl Host {
    #[must_use]
    pub fn new(worker_executable: PathBuf) -> Self {
        Self {
            worker_executable,
            store: Arc::new(Mutex::new(Store::new(
                StoreConfig::default(),
                Arc::new(RealClock),
            ))),
        }
    }

    pub fn run_script(&self, script: &str, timeout: Duration) -> Result<Json, WorkerError> {
        self.run_script_inner(script, timeout, None)
    }

    pub fn run_script_with_gate(
        &self,
        script: &str,
        timeout: Duration,
        gate: &Barrier,
    ) -> Result<Json, WorkerError> {
        self.run_script_inner(script, timeout, Some(gate))
    }

    fn run_script_inner(
        &self,
        script: &str,
        timeout: Duration,
        gate: Option<&Barrier>,
    ) -> Result<Json, WorkerError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| WorkerError::Protocol("timeout is too large".into()))?;
        let mut child = Command::new(&self.worker_executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| WorkerError::Terminated)?;
        let Some(mut stdin) = child.stdin.take() else {
            terminate(&mut child);
            return Err(WorkerError::Terminated);
        };
        let Some(stdout) = child.stdout.take() else {
            terminate(&mut child);
            return Err(WorkerError::Terminated);
        };
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || read_worker_lines(stdout, &sender));
        let job = ParentMessage::Job {
            script: script.to_string(),
            request: Json::Null,
        };
        if write_json_line(&mut stdin, &job).is_err() {
            terminate(&mut child);
            return Err(WorkerError::Terminated);
        }

        let exchange = self.exchange(&mut stdin, &receiver, deadline, gate);
        drop(stdin);
        let result = match exchange {
            Ok(result) => result,
            Err(error) => {
                terminate(&mut child);
                let _ = reader.join();
                return Err(error);
            }
        };
        let status = wait_until(&mut child, deadline);
        let _ = reader.join();
        let trailing_line = receiver.try_recv().ok();
        match status {
            Ok(true) if trailing_line.is_none() => result,
            Ok(true) => Err(WorkerError::Protocol(
                "worker wrote after final_result".into(),
            )),
            Ok(false) | Err(()) => Err(WorkerError::Terminated),
        }
    }

    fn exchange(
        &self,
        stdin: &mut std::process::ChildStdin,
        receiver: &mpsc::Receiver<Result<String, String>>,
        deadline: Instant,
        gate: Option<&Barrier>,
    ) -> Result<Result<Json, WorkerError>, WorkerError> {
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(WorkerError::Timeout)?;
            let line = receiver
                .recv_timeout(remaining)
                .map_err(|_| WorkerError::Timeout)?
                .map_err(WorkerError::Protocol)?;
            let message = serde_json::from_str::<WorkerMessage>(&line).map_err(|error| {
                WorkerError::Protocol(format!("invalid worker message: {error}"))
            })?;
            match message {
                WorkerMessage::HostCall { name, payload } => {
                    let BridgeName::State = name;
                    if let Some(gate) = gate {
                        gate.wait();
                    }
                    let result = dispatch_state(&self.store, &payload);
                    if write_json_line(
                        stdin,
                        &ParentMessage::HostResult {
                            result: result.clone(),
                        },
                    )
                    .is_err()
                    {
                        return Err(WorkerError::Terminated);
                    }
                }
                WorkerMessage::FinalResult { ok, body } => {
                    return parse_final_result(ok, body);
                }
            }
        }
    }
}

fn parse_final_result(ok: bool, body: FinalBody) -> Result<Result<Json, WorkerError>, WorkerError> {
    match (ok, body.result, body.error) {
        (true, Some(result), None) => {
            let responded = result
                .get("responded")
                .and_then(Json::as_bool)
                .ok_or_else(|| WorkerError::Protocol("missing responded flag".into()))?;
            if !responded {
                return Ok(Err(WorkerError::Script(
                    "script finished without ctx.respond".into(),
                )));
            }
            let response = result
                .get("response")
                .cloned()
                .ok_or_else(|| WorkerError::Protocol("missing response value".into()))?;
            Ok(Ok(response))
        }
        (false, None, Some(error)) => {
            let message = error
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or("script failed")
                .to_string();
            Ok(Err(WorkerError::Script(message)))
        }
        _ => Err(WorkerError::Protocol("malformed final_result shape".into())),
    }
}

fn dispatch_state(store: &Arc<Mutex<Store>>, payload: &Json) -> Json {
    let Some(operation) = payload.get("op").and_then(Json::as_str) else {
        return state_error_json(StateError::InvalidInput);
    };
    let Some(key) = payload.get("key").and_then(Json::as_str) else {
        return state_error_json(StateError::InvalidInput);
    };
    let ttl = match parse_ttl(payload) {
        Ok(ttl) => ttl,
        Err(error) => return state_error_json(error),
    };
    let Ok(mut store) = store.lock() else {
        return state_error_json(StateError::Internal);
    };
    let result = match operation {
        "get" => match store.get(key) {
            Ok(Some(value)) => json!({ "ok": true, "found": true, "value": value }),
            Ok(None) => json!({ "ok": true, "found": false }),
            Err(error) => state_error_json(error),
        },
        "set" => {
            let value = payload.get("value").cloned().map_or_else(
                || state_error_json(StateError::InvalidInput),
                |value| match store.set(key, value, ttl) {
                    Ok(()) => json!({ "ok": true }),
                    Err(error) => state_error_json(error),
                },
            );
            value
        }
        "take" => match store.take(key) {
            Ok(Some(value)) => json!({ "ok": true, "found": true, "value": value }),
            Ok(None) => json!({ "ok": true, "found": false }),
            Err(error) => state_error_json(error),
        },
        "setIfAbsent" => {
            let value = payload.get("value").cloned().map_or_else(
                || state_error_json(StateError::InvalidInput),
                |value| match store.set_if_absent(key, value, ttl) {
                    Ok(stored) => json!({ "ok": true, "stored": stored }),
                    Err(error) => state_error_json(error),
                },
            );
            value
        }
        _ => state_error_json(StateError::InvalidInput),
    };
    result
}

fn parse_ttl(payload: &Json) -> Result<Option<Duration>, StateError> {
    let Some(ttl) = payload.get("ttl_ms") else {
        return Ok(None);
    };
    let ttl = ttl
        .as_u64()
        .filter(|ttl| *ttl > 0 && *ttl <= MAX_TTL_MS)
        .ok_or(StateError::InvalidInput)?;
    Ok(Some(Duration::from_millis(ttl)))
}

fn state_error_json(error: StateError) -> Json {
    json!({
        "ok": false,
        "code": error.code(),
        "message": error.code(),
    })
}

fn write_json_line<T: Serialize>(writer: &mut impl Write, value: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    if line.len() > usize::try_from(MAX_PROTOCOL_LINE_BYTES).unwrap_or(usize::MAX) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "protocol line exceeded 64 KiB",
        ));
    }
    writer.write_all(&line)?;
    writer.flush()
}

fn read_worker_lines(
    stdout: std::process::ChildStdout,
    sender: &mpsc::Sender<Result<String, String>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_limited_line(&mut reader) {
            Ok(None) => return,
            Ok(Some(line)) => {
                if sender.send(Ok(line)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        }
    }
}

fn read_limited_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    let max = MAX_PROTOCOL_LINE_BYTES
        .checked_add(1)
        .unwrap_or(MAX_PROTOCOL_LINE_BYTES);
    let read = reader.by_ref().take(max).read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > usize::try_from(MAX_PROTOCOL_LINE_BYTES).unwrap_or(usize::MAX) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "protocol line exceeded 64 KiB",
        ));
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<bool, ()> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {}
            Err(_) => return Err(()),
        }
        if Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

const PRELUDE: &str = r#"
var __sd = { responded: false, response: null };
var ctx = (function (__sd_state) {
  "use strict";
  function makeError(message, code) {
    var error = new Error(String(message));
    error.code = code;
    return error;
  }
  function invalid(message) {
    throw makeError(message, "state_invalid_input");
  }
  function checkString(value) {
    for (var i = 0; i < value.length; i++) {
      var unit = value.charCodeAt(i);
      if (unit >= 0xD800 && unit <= 0xDBFF) {
        var next = i + 1 < value.length ? value.charCodeAt(i + 1) : -1;
        if (next < 0xDC00 || next > 0xDFFF) {
          invalid("ctx.state: strings must be well-formed UTF-16");
        }
        i += 1;
      } else if (unit >= 0xDC00 && unit <= 0xDFFF) {
        invalid("ctx.state: strings must be well-formed UTF-16");
      }
    }
  }
  function checkKey(key) {
    if (typeof key !== "string" || key.length === 0) {
      invalid("ctx.state: key must be a non-empty string");
    }
    checkString(key);
    return key;
  }
  function checkValue(value, seen, depth) {
    if (depth > 64) { invalid("ctx.state: value nesting is too deep"); }
    if (value === null) { return; }
    var kind = typeof value;
    if (kind === "boolean") { return; }
    if (kind === "string") {
      checkString(value);
      return;
    }
    if (kind === "number") {
      if (!Number.isFinite(value) || Object.is(value, -0)) {
        invalid("ctx.state: value contains a non-JSON number");
      }
      return;
    }
    if (kind !== "object") {
      invalid("ctx.state: value contains " + kind);
    }
    if (seen.indexOf(value) !== -1) {
      invalid("ctx.state: value contains a cycle");
    }
    seen.push(value);
    if (Array.isArray(value)) {
      if (Object.getOwnPropertySymbols(value).length !== 0) {
        invalid("ctx.state: symbol keys are not supported");
      }
      var names = Object.getOwnPropertyNames(value);
      for (var n = 0; n < names.length; n++) {
        if (names[n] === "length") { continue; }
        var index = Number(names[n]);
        var descriptor = Object.getOwnPropertyDescriptor(value, names[n]);
        if (!/^(0|[1-9][0-9]*)$/.test(names[n]) ||
            !Number.isInteger(index) || index < 0 || index >= value.length ||
            !descriptor || !("value" in descriptor) || !descriptor.enumerable) {
          invalid("ctx.state: arrays can only contain dense data elements");
        }
        checkValue(descriptor.value, seen, depth + 1);
      }
      for (var i = 0; i < value.length; i++) {
        if (!Object.prototype.hasOwnProperty.call(value, i)) {
          invalid("ctx.state: sparse arrays are not supported");
        }
      }
    } else {
      var proto = Object.getPrototypeOf(value);
      if (proto !== Object.prototype && proto !== null) {
        invalid("ctx.state: value must contain only plain objects");
      }
      if (Object.getOwnPropertySymbols(value).length !== 0) {
        invalid("ctx.state: symbol keys are not supported");
      }
      var keys = Object.getOwnPropertyNames(value);
      for (var k = 0; k < keys.length; k++) {
        checkString(keys[k]);
        var descriptor = Object.getOwnPropertyDescriptor(value, keys[k]);
        if (!descriptor || !("value" in descriptor) || !descriptor.enumerable) {
          invalid("ctx.state: accessors and hidden properties are not supported");
        }
        checkValue(descriptor.value, seen, depth + 1);
      }
    }
    seen.pop();
  }
  function checkTtl(opts) {
    if (opts === undefined) { return null; }
    if (opts === null || typeof opts !== "object" || Array.isArray(opts)) {
      invalid("ctx.state: opts must be a plain object");
    }
    var proto = Object.getPrototypeOf(opts);
    if (proto !== Object.prototype && proto !== null) {
      invalid("ctx.state: opts must be a plain object");
    }
    var keys = Object.getOwnPropertyNames(opts);
    for (var i = 0; i < keys.length; i++) {
      if (keys[i] !== "ttl_ms") {
        invalid("ctx.state: unknown opts key");
      }
    }
    if (!Object.prototype.hasOwnProperty.call(opts, "ttl_ms")) { return null; }
    var ttl = opts.ttl_ms;
    if (typeof ttl !== "number" || !Number.isInteger(ttl) || ttl <= 0 ||
        ttl > __SD_MAX_TTL_MS__) {
      invalid("ctx.state: opts.ttl_ms must be an integer in [1, __SD_MAX_TTL_MS__]");
    }
    return ttl;
  }
  function call(payload) {
    var raw;
    try {
      raw = __sd_state(JSON.stringify(payload));
    } catch (error) {
      throw makeError("ctx.state: host bridge failed", "state_internal_error");
    }
    var result;
    try {
      result = JSON.parse(raw);
    } catch (error) {
      throw makeError("ctx.state: invalid host response", "state_internal_error");
    }
    if (!result.ok) {
      throw makeError(result.message || "ctx.state failed",
        result.code || "state_internal_error");
    }
    return result;
  }
  function write(method, key, value, opts) {
    var checkedKey = checkKey(key);
    var seen = [];
    checkValue(value, seen, 0);
    var payload = { op: method, key: checkedKey, value: value };
    var ttl = checkTtl(opts);
    if (ttl !== null) { payload.ttl_ms = ttl; }
    return call(payload);
  }
  var state = Object.freeze({
    get: function (key) {
      var result = call({ op: "get", key: checkKey(key) });
      return result.found ? result.value : undefined;
    },
    set: function (key, value, opts) {
      write("set", key, value, opts);
    },
    take: function (key) {
      var result = call({ op: "take", key: checkKey(key) });
      return result.found ? result.value : undefined;
    },
    setIfAbsent: function (key, value, opts) {
      return write("setIfAbsent", key, value, opts).stored;
    }
  });
  return Object.freeze({
    apiVersion: "spike",
    state: state,
    respond: function (value) {
      __sd.response = value;
      __sd.responded = true;
      return true;
    }
  });
})(__sd_state);
delete globalThis.__sd_state;
"#;

const EXTRACT: &str = "JSON.stringify({ responded: __sd.responded, response: __sd.response })";

fn worker_state_bridge(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message("__sd_state: expected a JSON string")
            .into());
    };
    let payload: Json = serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
        JsNativeError::typ().with_message(format!("__sd_state: invalid payload: {error}"))
    })?;
    let call = WorkerMessage::HostCall {
        name: BridgeName::State,
        payload,
    };
    write_worker_line(&call).map_err(|error| {
        JsNativeError::error().with_message(format!("__sd_state: host write failed: {error}"))
    })?;
    let line = read_stdin_line().map_err(|error| {
        JsNativeError::error().with_message(format!("__sd_state: host read failed: {error}"))
    })?;
    let line = line
        .ok_or_else(|| JsNativeError::error().with_message("__sd_state: host closed the pipe"))?;
    let message: ParentMessage = serde_json::from_str(&line).map_err(|error| {
        JsNativeError::error().with_message(format!("__sd_state: invalid host response: {error}"))
    })?;
    let ParentMessage::HostResult { result } = message else {
        return Err(JsNativeError::error()
            .with_message("__sd_state: expected host_result")
            .into());
    };
    let rendered = serde_json::to_string(&result).map_err(|error| {
        JsNativeError::error().with_message(format!("__sd_state: cannot encode result: {error}"))
    })?;
    Ok(JsValue::from(JsString::from(rendered)))
}

fn evaluate(script: &str) -> Result<Json, String> {
    let mut context = Context::default();
    let prelude = PRELUDE.replace("__SD_MAX_TTL_MS__", &MAX_TTL_MS.to_string());
    context
        .register_global_builtin_callable(
            js_string!("__sd_state"),
            1,
            NativeFunction::from_fn_ptr(worker_state_bridge),
        )
        .map_err(|error| error.to_string())?;
    let evaluated = (|| -> Result<String, JsError> {
        context.eval(Source::from_bytes(&prelude))?;
        context.eval(Source::from_bytes(script))?;
        let dump = context.eval(Source::from_bytes(EXTRACT))?;
        Ok(dump
            .as_string()
            .map_or_else(|| "null".to_string(), |text| text.to_std_string_escaped()))
    })();
    match evaluated {
        Ok(dump) => serde_json::from_str(&dump).map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

fn write_worker_line(message: &WorkerMessage) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    write_json_line(&mut stdout, message)
}

fn read_stdin_line() -> std::io::Result<Option<String>> {
    let stdin = std::io::stdin();
    read_limited_line(&mut stdin.lock())
}

pub fn run_worker_process() -> std::io::Result<()> {
    let Some(line) = read_stdin_line()? else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "missing job line",
        ));
    };
    let message: ParentMessage = serde_json::from_str(&line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let ParentMessage::Job { script, .. } = message else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected a job line",
        ));
    };
    let final_message = match evaluate(&script) {
        Ok(result) => WorkerMessage::FinalResult {
            ok: true,
            body: FinalBody {
                result: Some(result),
                error: None,
            },
        },
        Err(message) => WorkerMessage::FinalResult {
            ok: false,
            body: FinalBody {
                result: None,
                error: Some(json!({ "message": message })),
            },
        },
    };
    write_worker_line(&final_message)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn protocol_reader_rejects_an_oversized_line() {
        let oversized = vec![b'x'; usize::try_from(MAX_PROTOCOL_LINE_BYTES).unwrap() + 1];
        let mut reader = Cursor::new(oversized);
        let error = read_limited_line(&mut reader).expect_err("line must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn protocol_writer_rejects_an_oversized_line() {
        let value = "x".repeat(usize::try_from(MAX_PROTOCOL_LINE_BYTES).unwrap());
        let mut output = Vec::new();
        let error = write_json_line(&mut output, &value).expect_err("line must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn poisoned_store_returns_a_catchable_internal_error_result() {
        let store = Arc::new(Mutex::new(Store::new(
            StoreConfig::default(),
            Arc::new(RealClock),
        )));
        let poisoning_store = Arc::clone(&store);
        let _ = thread::spawn(move || {
            let _guard = poisoning_store.lock().expect("poison store");
            panic!("poison the store lock");
        })
        .join();

        let result = dispatch_state(&store, &json!({ "op": "get", "key": "key" }));
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("state_internal_error"));
    }
}
