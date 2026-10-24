// Script execution: run one route script in a fresh worker process with a
// host-injected `ctx`. The parent owns every host capability and speaks a
// strict JSON Lines protocol with the worker over stdin/stdout.
//
// The crate denies `unsafe`. Boa 0.22 only exposes native closures through
// `unsafe fn NativeFunction::from_closure`, so the only Rust callback is a safe
// `NativeFunction::from_fn_ptr` bridge that forwards host calls to the parent.
// The prelude builds `ctx` in the realm from JSON snapshots and reads the
// produced response back with `JSON.stringify`. Scripts still see nothing but
// `ctx`: the realm has no `fetch`, `fs`, `process`, or `require`, and the raw
// bridge global is deleted before the route script runs.
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write as _};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsNativeErrorKind, JsString, JsValue, Source,
};
use serde::de;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value as Json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::{FilesConfig, SandboxConfig, UpstreamConfig, SCRIPT_MEMORY_LIMIT_FLOOR_MB};
use crate::crypto;
use crate::files;
use crate::matcher::percent_decode;
use crate::state::{self, SharedState};
use crate::upstream;

/// Value of `ctx.apiVersion`; see docs/contracts/ctx-api.md.
pub const API_VERSION: &str = "1";

/// Hidden subcommand that turns the server binary into one script worker.
pub const WORKER_SUBCOMMAND: &str = "__script-worker";

// One script run gets a fixed resource envelope. Boa 0.22 exposes no interrupt
// hook and no heap metric or limit, so the parent's wall-clock deadline and
// process kill are the enforced bound; the iteration limit, recursion limit,
// and VM stack limit remain defense in depth inside the disposable worker.
const LOOP_ITERATION_LIMIT: u64 = 100_000_000;
const RECURSION_LIMIT: usize = 512;
const VM_STACK_SIZE_LIMIT: usize = 10_240;

/// Native stack for the thread that runs Boa. Windows gives the process main
/// thread a 1 MiB stack, which Boa can exhaust during initialization or a
/// normal script; Linux and macOS happen to have enough. The worker is
/// disposable and already bounded by its deadline, so use one explicit size on
/// every platform instead of relying on the host default.
const SCRIPT_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

// Bridge payloads can carry an 8 MiB buffered value after base64 expansion or
// a script response body. Keep a finite ceiling well above those documented
// limits so a corrupt worker cannot grow one protocol allocation indefinitely.
const MAX_PROTOCOL_LINE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(unix)]
const SENTINEL_STACK_SIZE: usize = 256 * 1024;

/// Internal environment override that keeps glibc from reserving a per-thread
/// malloc arena for the sentinel.
const MALLOC_ARENA_MAX_ENV: &str = "MALLOC_ARENA_MAX";

/// Read-only view of one client request, frozen into `ctx.request`.
#[derive(Debug, Clone)]
pub struct RequestSnapshot {
    pub method: String,
    pub path: String,
    pub params: HashMap<String, String>,
    pub query: BTreeMap<String, String>,
    pub headers: Vec<(String, String)>,
    pub body_text: Option<String>,
    pub files: Vec<files::UploadFileMeta>,
}

impl RequestSnapshot {
    /// Build the script-visible snapshot. Header names are lowercased; a body
    /// that is not UTF-8 becomes `null` instead of a lossy string.
    #[must_use]
    pub fn new(
        method: &Method,
        path: &str,
        params: HashMap<String, String>,
        raw_query: &str,
        headers: &HeaderMap,
        body: Option<&[u8]>,
        files: Vec<files::UploadFileMeta>,
    ) -> Self {
        Self {
            method: method.as_str().to_ascii_uppercase(),
            path: path.to_string(),
            params,
            query: parse_query(raw_query),
            headers: headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_ascii_lowercase(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            body_text: body.and_then(|body| std::str::from_utf8(body).map(str::to_string).ok()),
            files,
        }
    }

    fn to_json(&self) -> Json {
        let params = Json::Object(
            self.params
                .iter()
                .map(|(key, value)| (key.clone(), Json::String(value.clone())))
                .collect(),
        );
        let query = Json::Object(
            self.query
                .iter()
                .map(|(key, value)| (key.clone(), Json::String(value.clone())))
                .collect(),
        );
        // tradeoff: repeated header names keep the last value; multi-value
        // headers need a list shape in a later `ctx` contract revision.
        let headers = Json::Object(
            self.headers
                .iter()
                .map(|(name, value)| (name.clone(), Json::String(value.clone())))
                .collect(),
        );
        json!({
            "method": self.method,
            "path": self.path,
            "params": params,
            "query": query,
            "headers": headers,
            "bodyText": self.body_text,
            "files": self.files.iter().map(files::UploadFileMeta::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Body of a script-produced response.
#[derive(Debug)]
pub enum ResponseBody {
    Text(String),
    Bytes(Vec<u8>),
    /// Streamed by `ctx.http.pipe`: frames move from the upstream connection
    /// to the client without entering the JavaScript heap.
    Stream(upstream::PipeBody),
    /// Streamed by `ctx.file.stream`: the opened file is relayed to the client
    /// without entering the JavaScript heap; the host owns framing headers.
    File(files::FileBody),
}

/// Response produced by `ctx.respond` or `ctx.http.pipe`.
#[derive(Debug)]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ResponseBody,
}

/// One `ctx.log.*` call. Reaches server logs only, never a client body.
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub level: String,
    pub message: String,
}

/// Why a script did not produce a usable response.
#[derive(Debug)]
pub enum Error {
    /// Script threw, the engine aborted, or the response could not be
    /// extracted. A vanished worker maps to [`Error::WorkerTerminated`].
    Failed(String),
    /// Wall-clock deadline exceeded.
    TimedOut,
    /// The worker exceeded the configured memory bound: virtual address space
    /// on Linux (`RLIMIT_AS`), commit charge on Windows (Job Object).
    MemoryLimitExceeded,
    /// Every script worker slot is busy; the host failed fast instead of
    /// letting queued scripts wait behind running workers.
    CapacityExceeded,
    /// The worker process ended before delivering a complete final result.
    WorkerTerminated,
    /// Script finished without calling `ctx.respond`.
    NoResponse,
    /// An uncaught transport failure from `ctx.http.get` or `ctx.http.pipe`.
    UpstreamUnreachable {
        message: String,
        /// Stable kind: `"timeout"`, `"dns"`, or `"transport"`.
        kind: &'static str,
    },
}

impl Error {
    /// Stable error class used in responses and logs.
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            Self::NoResponse => "script_no_response",
            Self::Failed(_)
            | Self::TimedOut
            | Self::MemoryLimitExceeded
            | Self::CapacityExceeded
            | Self::WorkerTerminated => "script_error",
            Self::UpstreamUnreachable { .. } => "upstream_unreachable",
        }
    }

    /// HTTP status used when this error reaches the client uncaught.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::UpstreamUnreachable { .. } => StatusCode::BAD_GATEWAY,
            Self::Failed(_)
            | Self::TimedOut
            | Self::MemoryLimitExceeded
            | Self::CapacityExceeded
            | Self::WorkerTerminated
            | Self::NoResponse => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Client-visible diagnostic under `--verbose`: a stable class only, never
    /// a stack trace, script message, upstream body, or upstream address.
    #[must_use]
    pub fn detail(&self) -> &'static str {
        match self {
            Self::Failed(_) => "script execution failed",
            Self::TimedOut => "script exceeded the configured timeout",
            Self::MemoryLimitExceeded => "script exceeded the configured memory limit",
            Self::CapacityExceeded => "script worker capacity exhausted",
            Self::WorkerTerminated => "script worker terminated unexpectedly",
            Self::NoResponse => "script finished without calling ctx.respond",
            Self::UpstreamUnreachable { kind, .. } => match *kind {
                "timeout" => "upstream transport failure: timeout",
                "dns" => "upstream transport failure: dns",
                _ => "upstream transport failure: transport",
            },
        }
    }
}

/// Everything one script run produced.
#[derive(Debug)]
pub struct Outcome {
    pub response: Option<ScriptResponse>,
    pub logs: Vec<LogRecord>,
    pub error: Option<Error>,
    /// Ordered upstream calls made by this script run.
    pub upstream_calls: Vec<Json>,
    /// Shared file call chain, finalized as each Response is mapped.
    pub file_calls: files::CallLog,
}

impl Outcome {
    /// Report a failure that happened before the script could run.
    #[must_use]
    pub fn failed(error: Error) -> Self {
        Self {
            response: None,
            logs: Vec::new(),
            error: Some(error),
            upstream_calls: Vec::new(),
            file_calls: files::CallLog::default(),
        }
    }
}

// The prelude is the only writer of `__sd`; `ctx` is a frozen view over it.
const PRELUDE: &str = r#"
var __sd = { response: null, logs: [] };
var ctx = (function (__sd_http_get, __sd_http_pipe, __sd_validate_headers, __sd_upstream_marker, __sd_file, __sd_upload, __sd_state, __sd_crypto, __sd_intrinsics) {
  "use strict";
  var envelope = __sd;
  var __sd_json_stringify = __sd_intrinsics.json_stringify;
  var __sd_json_parse = __sd_intrinsics.json_parse;
  var __sd_get_own_property_names = __sd_intrinsics.get_own_property_names;
  var __sd_get_own_property_symbols = __sd_intrinsics.get_own_property_symbols;
  var __sd_get_own_property_descriptor = __sd_intrinsics.get_own_property_descriptor;
  var __sd_get_prototype_of = __sd_intrinsics.get_prototype_of;
  var __sd_object_prototype = __sd_intrinsics.object_prototype;
  var __sd_has_own = __sd_intrinsics.object_has_own;
  var __sd_reflect_apply = __sd_intrinsics.reflect_apply;
  var __sd_array_is_array = __sd_intrinsics.array_is_array;
  var __sd_number_is_integer = __sd_intrinsics.number_is_integer;
  var __sd_number_is_finite = __sd_intrinsics.number_is_finite;
  var __sd_number_is_safe_integer = __sd_intrinsics.number_is_safe_integer;
  var __sd_number = __sd_intrinsics.number;
  var __sd_object_is = __sd_intrinsics.object_is;
  var __sd_regexp_test = __sd_intrinsics.regexp_test;
  var FILE_HANDLES = new WeakMap();
  function hasOwn(object, key) {
    return __sd_reflect_apply(__sd_has_own, object, [key]);
  }
  function format(value) {
    if (typeof value === "string") { return value; }
    if (value === undefined) { return "undefined"; }
    if (value === null) { return "null"; }
    try { return JSON.stringify(value); } catch (e) { return String(value); }
  }
  function record(level, args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) { parts.push(format(args[i])); }
    envelope.logs.push({ level: level, message: parts.join(" ") });
  }
  function checkStatus(status, label) {
    if (typeof status !== "number" || !Number.isInteger(status) || status < 100 || status > 599) {
      throw new TypeError(label + ": status must be an integer in [100, 599]");
    }
    return status;
  }
  function checkHeaders(headers, label) {
    var out = [];
    if (headers === undefined || headers === null) { return out; }
    if (Array.isArray(headers)) {
      for (var i = 0; i < headers.length; i++) {
        var row = headers[i];
        if (!Array.isArray(row) || row.length !== 2) {
          throw new TypeError(label + ": headers[" + i + "] must be a [name, value] pair");
        }
        out.push([String(row[0]), String(row[1])]);
      }
    } else if (typeof headers !== "object") {
      throw new TypeError(label + ": headers must be an object or [name, value] pairs");
    } else {
      var names = Object.keys(headers);
      for (var j = 0; j < names.length; j++) {
        var name = names[j];
        var value = headers[name];
        if (Array.isArray(value)) {
          for (var k = 0; k < value.length; k++) { out.push([name, String(value[k])]); }
        } else {
          out.push([name, String(value)]);
        }
      }
    }
    var raw;
    try {
      raw = __sd_validate_headers(JSON.stringify(out));
    } catch (bridgeError) {
      throw makeError(label + ": host bridge failed", "script_error");
    }
    var result;
    try {
      result = JSON.parse(raw);
    } catch (parseError) {
      throw makeError(label + ": host bridge failed", "script_error");
    }
    if (!result.ok) {
      throw makeError(label + ": " + result.message, result.code || "script_error");
    }
    return out;
  }
  function checkBody(body) {
    if (body === undefined || body === null) { return ""; }
    if (typeof body === "string") { return body; }
    if (typeof body === "object") {
      var mapped = FILE_HANDLES.get(body);
      if (mapped !== undefined) { return mapped; }
    }
    var bytes;
    if (body instanceof Uint8Array) {
      bytes = Array.prototype.slice.call(body);
    } else if (body instanceof ArrayBuffer) {
      bytes = Array.prototype.slice.call(new Uint8Array(body));
    } else if (Array.isArray(body)) {
      bytes = body;
    } else {
      throw new TypeError("ctx.respond: body must be a string, byte array, or Uint8Array");
    }
    for (var i = 0; i < bytes.length; i++) {
      var byte = bytes[i];
      if (!Number.isInteger(byte) || byte < 0 || byte > 255) {
        throw new TypeError("ctx.respond: body bytes must be integers in [0, 255]");
      }
    }
    return bytes;
  }
  function freezeShallow(value) {
    Object.keys(value).forEach(function (key) {
      if (value[key] && typeof value[key] === "object") { Object.freeze(value[key]); }
    });
    return Object.freeze(value);
  }
  function makeError(message, code) {
    var error = new Error(String(message));
    error.code = code;
    return error;
  }
  function makeUpstreamError(message, kind) {
    var error = makeError(message, "upstream_unreachable");
    Object.defineProperty(error, __sd_upstream_marker, { value: kind || "transport" });
    return error;
  }
  var BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function decodeBase64(text) {
    var clean = String(text).replace(/=+$/, "");
    var out = new Uint8Array(Math.floor(clean.length * 3 / 4));
    var buffer = 0;
    var bits = 0;
    var index = 0;
    for (var i = 0; i < clean.length; i++) {
      var value = BASE64_ALPHABET.indexOf(clean.charAt(i));
      if (value < 0) { throw new TypeError("invalid base64 body"); }
      buffer = (buffer << 6) | value;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out[index] = (buffer >> bits) & 0xff;
        index += 1;
      }
    }
    return out;
  }
  function encodeBase64(bytes) {
    var len = bytes.length;
    var extra = len % 3;
    var mainLen = len - extra;
    var parts = [];
    var chunk = "";
    for (var i = 0; i < mainLen; i += 3) {
      var b0 = bytes[i];
      var b1 = bytes[i + 1];
      var b2 = bytes[i + 2];
      chunk += BASE64_ALPHABET.charAt(b0 >> 2) +
               BASE64_ALPHABET.charAt(((b0 & 3) << 4) | (b1 >> 4)) +
               BASE64_ALPHABET.charAt(((b1 & 15) << 2) | (b2 >> 6)) +
               BASE64_ALPHABET.charAt(b2 & 63);
      if (chunk.length >= 8192) {
        parts.push(chunk);
        chunk = "";
      }
    }
    if (chunk.length > 0) { parts.push(chunk); }
    if (extra === 1) {
      var b0 = bytes[mainLen];
      parts.push(BASE64_ALPHABET.charAt(b0 >> 2) + BASE64_ALPHABET.charAt((b0 & 3) << 4) + "==");
    } else if (extra === 2) {
      var b0 = bytes[mainLen];
      var b1 = bytes[mainLen + 1];
      parts.push(BASE64_ALPHABET.charAt(b0 >> 2) + BASE64_ALPHABET.charAt(((b0 & 3) << 4) | (b1 >> 4)) + BASE64_ALPHABET.charAt((b1 & 15) << 2) + "=");
    }
    return parts.join("");
  }
  function checkPlainOpts(opts, label, allowedKeys) {
    if (opts === undefined) { return {}; }
    if (opts === null || typeof opts !== "object" || __sd_array_is_array(opts)) {
      throw new TypeError(label + ": opts must be an object");
    }
    var proto = __sd_get_prototype_of(opts);
    if (proto !== __sd_object_prototype && proto !== null) {
      throw new TypeError(label + ": opts must be a plain object");
    }
    var symbols = __sd_get_own_property_symbols(opts);
    if (symbols.length > 0) {
      throw new TypeError(label + ": unknown opts key " + String(symbols[0]));
    }
    var keys = __sd_get_own_property_names(opts);
    for (var i = 0; i < keys.length; i++) {
      if (allowedKeys.indexOf(keys[i]) === -1) {
        throw new TypeError(label + ": unknown opts key " + JSON.stringify(keys[i]));
      }
    }
    return opts;
  }
  function callBridge(bridge, payload, label) {
    var raw;
    try {
      raw = bridge(JSON.stringify(payload));
    } catch (bridgeError) {
      throw makeError(label + ": host bridge failed", "script_error");
    }
    var result = JSON.parse(raw);
    if (!result.ok) {
      if (result.code === "upstream_unreachable") {
        throw makeUpstreamError(result.message, result.kind);
      }
      throw makeError(result.message, result.code);
    }
    return result;
  }
  function invalidState(message) {
    throw makeError(message, "state_invalid_input");
  }
  function checkStateString(value) {
    for (var i = 0; i < value.length; i++) {
      var unit = value.charCodeAt(i);
      if (unit >= 0xD800 && unit <= 0xDBFF) {
        var next = i + 1 < value.length ? value.charCodeAt(i + 1) : -1;
        if (next < 0xDC00 || next > 0xDFFF) {
          invalidState("ctx.state: strings must be well-formed UTF-16");
        }
        i += 1;
      } else if (unit >= 0xDC00 && unit <= 0xDFFF) {
        invalidState("ctx.state: strings must be well-formed UTF-16");
      }
    }
  }
  function stateUtf8Length(value) {
    var bytes = 0;
    for (var i = 0; i < value.length; i++) {
      var unit = value.charCodeAt(i);
      if (unit <= 0x7F) {
        bytes += 1;
      } else if (unit <= 0x7FF) {
        bytes += 2;
      } else if (unit >= 0xD800 && unit <= 0xDBFF) {
        bytes += 4;
        i += 1;
      } else {
        bytes += 3;
      }
    }
    return bytes;
  }
  function checkStateValue(value, seen, depth) {
    if (depth > __SD_STATE_MAX_DEPTH__) {
      invalidState("ctx.state: value nesting is too deep");
    }
    if (value === null) { return; }
    var kind = typeof value;
    if (kind === "boolean") { return; }
    if (kind === "string") {
      checkStateString(value);
      return;
    }
    if (kind === "number") {
      if (!__sd_number_is_finite(value) || __sd_object_is(value, -0)) {
        invalidState("ctx.state: value contains a non-JSON number");
      }
      return;
    }
    if (kind !== "object") {
      invalidState("ctx.state: value contains " + kind);
    }
    if (seen.indexOf(value) !== -1) {
      invalidState("ctx.state: value contains a cycle");
    }
    seen.push(value);
    if (__sd_array_is_array(value)) {
      if (__sd_get_own_property_symbols(value).length !== 0) {
        invalidState("ctx.state: symbol keys are not supported");
      }
      var names = __sd_get_own_property_names(value);
      for (var n = 0; n < names.length; n++) {
        if (names[n] === "length") { continue; }
        var index = __sd_number(names[n]);
        var descriptor = __sd_get_own_property_descriptor(value, names[n]);
        if (!__sd_reflect_apply(__sd_regexp_test, /^(0|[1-9][0-9]*)$/, [names[n]]) ||
            !__sd_number_is_integer(index) || index < 0 || index >= value.length ||
            !descriptor || !("value" in descriptor) || !descriptor.enumerable) {
          invalidState("ctx.state: arrays can only contain dense data elements");
        }
        checkStateValue(descriptor.value, seen, depth + 1);
      }
      for (var arrayIndex = 0; arrayIndex < value.length; arrayIndex++) {
        if (!hasOwn(value, arrayIndex)) {
          invalidState("ctx.state: sparse arrays are not supported");
        }
      }
    } else {
      var proto = __sd_get_prototype_of(value);
      if (proto !== __sd_object_prototype && proto !== null) {
        invalidState("ctx.state: value must contain only plain objects");
      }
      if (__sd_get_own_property_symbols(value).length !== 0) {
        invalidState("ctx.state: symbol keys are not supported");
      }
      var keys = __sd_get_own_property_names(value);
      for (var k = 0; k < keys.length; k++) {
        checkStateString(keys[k]);
        var property = __sd_get_own_property_descriptor(value, keys[k]);
        if (!property || !("value" in property) || !property.enumerable) {
          invalidState("ctx.state: accessors and hidden properties are not supported");
        }
        checkStateValue(property.value, seen, depth + 1);
      }
    }
    seen.pop();
  }
  function checkStateKey(key) {
    if (typeof key !== "string" || key.length === 0) {
      throw makeError("ctx.state: key must be a non-empty string", "state_invalid_input");
    }
    checkStateString(key);
    if (stateUtf8Length(key) > __SD_STATE_MAX_KEY_BYTES__) {
      throw makeError(
        "ctx.state: key exceeds __SD_STATE_MAX_KEY_BYTES__ UTF-8 bytes",
        "state_invalid_input"
      );
    }
    return key;
  }
  function checkStateTtl(opts) {
    if (opts === undefined) { return null; }
    if (opts === null || typeof opts !== "object" || __sd_array_is_array(opts)) {
      throw makeError("ctx.state: opts must be a plain object", "state_invalid_input");
    }
    var proto = __sd_get_prototype_of(opts);
    if (proto !== __sd_object_prototype && proto !== null) {
      throw makeError("ctx.state: opts must be a plain object", "state_invalid_input");
    }
    if (__sd_get_own_property_symbols(opts).length !== 0) {
      throw makeError("ctx.state: unknown opts key", "state_invalid_input");
    }
    var keys = __sd_get_own_property_names(opts);
    for (var i = 0; i < keys.length; i++) {
      if (keys[i] !== "ttl_ms") {
        throw makeError("ctx.state: unknown opts key", "state_invalid_input");
      }
    }
    if (!hasOwn(opts, "ttl_ms")) { return null; }
    var ttl = opts.ttl_ms;
    if (typeof ttl !== "number" || !__sd_number_is_safe_integer(ttl) || ttl <= 0 ||
        ttl > __SD_STATE_MAX_TTL_MS__) {
      throw makeError(
        "ctx.state: opts.ttl_ms must be an integer in [1, __SD_STATE_MAX_TTL_MS__]",
        "state_invalid_input"
      );
    }
    return ttl;
  }
  function stateCall(payload) {
    var raw;
    try {
      raw = __sd_state(__sd_json_stringify(payload));
    } catch (error) {
      throw makeError("ctx.state: host bridge failed", "state_internal_error");
    }
    var result;
    try {
      result = __sd_json_parse(raw);
    } catch (error) {
      throw makeError("ctx.state: invalid host response", "state_internal_error");
    }
    if (!result.ok) {
      throw makeError(
        result.message || "ctx.state failed",
        result.code || "state_internal_error"
      );
    }
    return result;
  }
  function writeState(operation, key, value, opts) {
    checkStateValue(value, [], 0);
    var payload = {
      op: operation,
      key: checkStateKey(key),
      value: value
    };
    var ttl = checkStateTtl(opts);
    if (ttl !== null) { payload.ttl_ms = ttl; }
    return stateCall(payload);
  }
  var sharedState = Object.freeze({
    get: function (key) {
      var result = stateCall({ op: "get", key: checkStateKey(key) });
      return result.found ? result.value : undefined;
    },
    set: function (key, value, opts) {
      writeState("set", key, value, opts);
    },
    take: function (key) {
      var result = stateCall({ op: "take", key: checkStateKey(key) });
      return result.found ? result.value : undefined;
    },
    setIfAbsent: function (key, value, opts) {
      var result = writeState("setIfAbsent", key, value, opts);
      return result.stored;
    }
  });
  function checkCryptoString(value, label) {
    for (var i = 0; i < value.length; i++) {
      var unit = value.charCodeAt(i);
      if (unit >= 0xD800 && unit <= 0xDBFF) {
        var next = i + 1 < value.length ? value.charCodeAt(i + 1) : -1;
        if (next < 0xDC00 || next > 0xDFFF) {
          throw makeError(label + ": strings must not contain lone surrogates", "crypto_invalid_input");
        }
        i += 1;
      } else if (unit >= 0xDC00 && unit <= 0xDFFF) {
        throw makeError(label + ": strings must not contain lone surrogates", "crypto_invalid_input");
      }
    }
  }
  function cryptoUtf8Length(value) {
    var bytes = 0;
    for (var i = 0; i < value.length; i++) {
      var unit = value.charCodeAt(i);
      if (unit <= 0x7F) {
        bytes += 1;
      } else if (unit <= 0x7FF) {
        bytes += 2;
      } else if (unit >= 0xD800 && unit <= 0xDBFF) {
        bytes += 4;
        i += 1;
      } else {
        bytes += 3;
      }
    }
    return bytes;
  }
  function checkCryptoInput(value, label, minBytes, maxBytes) {
    if (value === undefined || value === null) {
      throw makeError(label + ": input is required", "crypto_invalid_input");
    }
    if (typeof value === "string") {
      checkCryptoString(value, label);
      var len = cryptoUtf8Length(value);
      if (minBytes !== undefined && len < minBytes) {
        throw makeError(label + ": input length below minimum", "crypto_invalid_input");
      }
      if (len > maxBytes) {
        throw makeError(label + ": input length exceeds maximum", "crypto_invalid_input");
      }
      return { type: "string", value: value };
    }
    var bytes = null;
    if (value instanceof Uint8Array) {
      bytes = value;
    } else if (value instanceof ArrayBuffer) {
      bytes = new Uint8Array(value);
    } else if (__sd_array_is_array(value)) {
      var arrLen = value.length;
      for (var i = 0; i < arrLen; i++) {
        if (!hasOwn(value, i)) {
          throw makeError(label + ": sparse arrays are not allowed", "crypto_invalid_input");
        }
        var item = value[i];
        if (typeof item !== "number" || !__sd_number_is_integer(item) || item < 0 || item > 255) {
          throw makeError(label + ": array elements must be integers in [0, 255]", "crypto_invalid_input");
        }
      }
      bytes = value;
    } else {
      throw makeError(label + ": unsupported input type", "crypto_invalid_input");
    }
    if (minBytes !== undefined && bytes.length < minBytes) {
      throw makeError(label + ": input length below minimum", "crypto_invalid_input");
    }
    if (bytes.length > maxBytes) {
      throw makeError(label + ": input length exceeds maximum", "crypto_invalid_input");
    }
    return { type: "bytes", value: encodeBase64(bytes) };
  }
  function cryptoRandomBytes(byteLength) {
    if (arguments.length < 1 || typeof byteLength !== "number" || !__sd_number_is_integer(byteLength) || byteLength < 1 || byteLength > 1024) {
      throw makeError("ctx.crypto.randomBytes: byteLength must be an integer in [1, 1024]", "crypto_invalid_input");
    }
    var result = callBridge(__sd_crypto, { op: "random_bytes", byte_length: byteLength }, "ctx.crypto.randomBytes");
    return decodeBase64(result.bytes_base64);
  }
  function cryptoSha256(input) {
    if (arguments.length < 1) {
      throw makeError("ctx.crypto.sha256: input is required", "crypto_invalid_input");
    }
    var checked = checkCryptoInput(input, "ctx.crypto.sha256", 0, 8388608);
    var result = callBridge(__sd_crypto, { op: "sha256", input: checked }, "ctx.crypto.sha256");
    return decodeBase64(result.bytes_base64);
  }
  function cryptoHmacSha256(key, message) {
    if (arguments.length < 2) {
      throw makeError("ctx.crypto.hmacSha256: key and message are required", "crypto_invalid_input");
    }
    var checkedKey = checkCryptoInput(key, "ctx.crypto.hmacSha256", 0, 1024);
    var checkedMessage = checkCryptoInput(message, "ctx.crypto.hmacSha256", 0, 8388608);
    var result = callBridge(__sd_crypto, { op: "hmac_sha256", key: checkedKey, message: checkedMessage }, "ctx.crypto.hmacSha256");
    return decodeBase64(result.bytes_base64);
  }
  function cryptoTimingSafeEqual(left, right) {
    if (arguments.length < 2) {
      throw makeError("ctx.crypto.timingSafeEqual: left and right are required", "crypto_invalid_input");
    }
    var checkedLeft = checkCryptoInput(left, "ctx.crypto.timingSafeEqual", 1, 1024);
    var checkedRight = checkCryptoInput(right, "ctx.crypto.timingSafeEqual", 1, 1024);
    var result = callBridge(__sd_crypto, { op: "timing_safe_equal", left: checkedLeft, right: checkedRight }, "ctx.crypto.timingSafeEqual");
    return result.equal;
  }
  function httpGet(url, opts) {
    if (typeof url !== "string") {
      throw new TypeError("ctx.http.get: url must be a string");
    }
    var checked = checkPlainOpts(opts, "ctx.http.get", ["timeout_ms"]);
    var timeout = null;
    if (Object.prototype.hasOwnProperty.call(checked, "timeout_ms")) {
      var value = checked.timeout_ms;
      if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) {
        throw new TypeError("ctx.http.get: opts.timeout_ms must be a positive integer");
      }
      timeout = value;
    }
    var payload = { url: url };
    if (timeout !== null) { payload.timeout_ms = timeout; }
    var result = callBridge(__sd_http_get, payload, "ctx.http.get");
    var upstream = result.response;
    return Object.freeze({
      status: upstream.status,
      headers: Object.freeze(upstream.headers),
      text: function () { return upstream.text; },
      bytes: function () { return decodeBase64(upstream.body_base64); }
    });
  }
  function checkFilePath(path, label) {
    if (typeof path !== "string") {
      throw new TypeError(label + ": path must be a string");
    }
    return path;
  }
  function fileCall(op, path, label) {
    return callBridge(__sd_file, { op: op, path: checkFilePath(path, label) }, label);
  }
  function fileReadText(path) {
    return fileCall("readText", path, "ctx.file.readText").text;
  }
  function fileReadBytes(path) {
    return decodeBase64(fileCall("readBytes", path, "ctx.file.readBytes").bytes_base64);
  }
  function fileStream(path) {
    var result = fileCall("stream", path, "ctx.file.stream");
    var handle = Object.freeze(Object.create(null));
    FILE_HANDLES.set(handle, { file: result.handle });
    return handle;
  }
  function uploadCall(op, index, label) {
    return callBridge(__sd_upload, { op: op, index: index }, label);
  }
  function uploadFile(meta, index) {
    var label = "ctx.request.files[" + index + "]";
    return Object.freeze({
      field: meta.field,
      filename: meta.filename,
      contentType: meta.contentType,
      size: meta.size,
      text: function () { return uploadCall("readText", index, label + ".text").text; },
      bytes: function () {
        return decodeBase64(uploadCall("readBytes", index, label + ".bytes").bytes_base64);
      },
      stream: function () {
        var result = uploadCall("stream", index, label + ".stream");
        var handle = Object.freeze(Object.create(null));
        FILE_HANDLES.set(handle, { upload: result.handle });
        return handle;
      }
    });
  }
  function requestSnapshot(raw) {
    var mapped = [];
    var rawFiles = raw.files || [];
    for (var i = 0; i < rawFiles.length; i++) { mapped.push(uploadFile(rawFiles[i], i)); }
    raw.files = mapped;
    return freezeShallow(raw);
  }
  var FILE_FRAMING_HEADERS = ["content-length", "content-range", "accept-ranges"];
  function checkFileStreamResponse(status, headers) {
    if (status !== 200) {
      throw makeError("ctx.respond: a file stream body requires status 200", "script_error");
    }
    for (var i = 0; i < headers.length; i++) {
      var name = String(headers[i][0]).toLowerCase();
      if (FILE_FRAMING_HEADERS.indexOf(name) !== -1) {
        throw makeError("ctx.respond: the host owns " + name + " for file streams", "script_error");
      }
    }
  }
  function checkPipeOpts(opts) {
    var out = { status: null, headers: [] };
    var checked = checkPlainOpts(opts, "ctx.http.pipe", ["status", "headers"]);
    if (Object.prototype.hasOwnProperty.call(checked, "status")) {
      out.status = checkStatus(checked.status, "ctx.http.pipe");
    }
    if (Object.prototype.hasOwnProperty.call(checked, "headers")) {
      out.headers = checkHeaders(checked.headers, "ctx.http.pipe");
    }
    return out;
  }
  function httpPipe(url, opts) {
    if (envelope.response !== null) {
      record("warn", ["ctx.http.pipe ignored: a response was already produced"]);
      return false;
    }
    if (typeof url !== "string") {
      throw new TypeError("ctx.http.pipe: url must be a string");
    }
    var checked = checkPipeOpts(opts);
    var payload = { url: url, headers: checked.headers };
    if (checked.status !== null) { payload.status = checked.status; }
    var result = callBridge(__sd_http_pipe, payload, "ctx.http.pipe");
    envelope.response = {
      stream: true,
      status: result.status,
      headers: result.headers
    };
    return true;
  }
  return Object.freeze({
    apiVersion: "__SD_API_VERSION__",
    request: requestSnapshot(__sd_request),
    env: Object.freeze(__sd_env),
    http: Object.freeze({ get: httpGet, pipe: httpPipe }),
    state: sharedState,
    crypto: Object.freeze({
      randomBytes: cryptoRandomBytes,
      sha256: cryptoSha256,
      hmacSha256: cryptoHmacSha256,
      timingSafeEqual: cryptoTimingSafeEqual
    }),
    file: Object.freeze({
      readText: fileReadText,
      readBytes: fileReadBytes,
      stream: fileStream
    }),
    log: Object.freeze({
      info: function () { record("info", arguments); },
      warn: function () { record("warn", arguments); },
      error: function () { record("error", arguments); }
    }),
    respond: function (status, headers, body) {
      if (envelope.response !== null) {
        record("warn", ["ctx.respond ignored: a response was already produced"]);
        return false;
      }
      var checkedStatus = checkStatus(status, "ctx.respond");
      var checkedHeaders = checkHeaders(headers, "ctx.respond");
      var checkedBody = checkBody(body);
      if (typeof checkedBody === "object" && checkedBody !== null &&
          (checkedBody.file !== undefined || checkedBody.upload !== undefined)) {
        checkFileStreamResponse(checkedStatus, checkedHeaders);
        FILE_HANDLES.delete(body);
      }
      envelope.response = {
        status: checkedStatus,
        headers: checkedHeaders,
        body: checkedBody
      };
      return true;
    }
  });
})(__sd_http_get, __sd_http_pipe, __sd_validate_headers, __sd_upstream_marker, __sd_file, __sd_upload, __sd_state, __sd_crypto, {
  json_stringify: JSON.stringify,
  json_parse: JSON.parse,
  get_own_property_names: Object.getOwnPropertyNames,
  get_own_property_symbols: Object.getOwnPropertySymbols,
  get_own_property_descriptor: Object.getOwnPropertyDescriptor,
  get_prototype_of: Object.getPrototypeOf,
  object_prototype: Object.prototype,
  object_has_own: Object.prototype.hasOwnProperty,
  reflect_apply: Reflect.apply,
  array_is_array: Array.isArray,
  number_is_integer: Number.isInteger,
  number_is_finite: Number.isFinite,
  number_is_safe_integer: Number.isSafeInteger,
  number: Number,
  object_is: Object.is,
  regexp_test: RegExp.prototype.test
});
delete globalThis.__sd_request;
delete globalThis.__sd_env;
delete globalThis.__sd_upstream_marker;
delete globalThis.__sd_http_get;
delete globalThis.__sd_http_pipe;
delete globalThis.__sd_validate_headers;
delete globalThis.__sd_file;
delete globalThis.__sd_upload;
delete globalThis.__sd_state;
delete globalThis.__sd_crypto;
"#;

/// Read the recorded response and log lines back out of the realm.
const EXTRACT: &str = "JSON.stringify({ response: __sd.response, logs: __sd.logs })";

/// One host bridge callable from the worker. Unknown names are protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum BridgeName {
    #[serde(rename = "__sd_http_get")]
    HttpGet,
    #[serde(rename = "__sd_http_pipe")]
    HttpPipe,
    #[serde(rename = "__sd_file")]
    File,
    #[serde(rename = "__sd_upload")]
    Upload,
    #[serde(rename = "__sd_state")]
    State,
    #[serde(rename = "__sd_validate_headers")]
    ValidateHeaders,
    #[serde(rename = "__sd_crypto")]
    Crypto,
}

impl BridgeName {
    const fn as_str(self) -> &'static str {
        match self {
            Self::HttpGet => "__sd_http_get",
            Self::HttpPipe => "__sd_http_pipe",
            Self::File => "__sd_file",
            Self::Upload => "__sd_upload",
            Self::State => "__sd_state",
            Self::ValidateHeaders => "__sd_validate_headers",
            Self::Crypto => "__sd_crypto",
        }
    }
}

/// Parent-to-worker JSON Lines messages.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ParentMessage {
    Job { script: String, request: Json },
    HostResult { result: Json },
}

/// Worker-to-parent JSON Lines messages.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerMessage {
    HostCall { name: BridgeName, payload: Json },
    FinalResult(FinalResult),
}

/// Stable error kind carried by a failed `final_result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FinalErrorKind {
    MemoryLimit,
    Timeout,
    Dns,
    Transport,
}

impl FinalErrorKind {
    const fn upstream_str(self) -> Option<&'static str> {
        match self {
            Self::MemoryLimit => None,
            Self::Timeout => Some("timeout"),
            Self::Dns => Some("dns"),
            Self::Transport => Some("transport"),
        }
    }

    fn from_upstream(kind: &str) -> Option<Self> {
        [Self::Timeout, Self::Dns, Self::Transport]
            .into_iter()
            .find(|candidate| candidate.upstream_str() == Some(kind))
    }

    fn into_error(self, message: String) -> Error {
        match self {
            Self::MemoryLimit => Error::MemoryLimitExceeded,
            Self::Timeout | Self::Dns | Self::Transport => Error::UpstreamUnreachable {
                message,
                kind: self.upstream_str().expect("upstream kind"),
            },
        }
    }
}

/// Failed `final_result` payload. An absent kind is a script failure; a known
/// kind selects the stable host-side error class.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct FinalError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<FinalErrorKind>,
    message: String,
}

impl FinalError {
    fn into_error(self) -> Error {
        match self.kind {
            Some(kind) => kind.into_error(self.message),
            None => Error::Failed(self.message),
        }
    }
}

/// The two valid `final_result` shapes. Parsing rejects success/error mixtures
/// and omissions at the protocol boundary.
#[derive(Debug, PartialEq)]
enum FinalResult {
    Success(Json),
    Error(FinalError),
}

#[derive(Deserialize)]
struct FinalResultWire {
    ok: bool,
    #[serde(flatten)]
    fields: serde_json::Map<String, Json>,
}

impl<'de> Deserialize<'de> for FinalResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut wire = FinalResultWire::deserialize(deserializer)?;
        let has_result = wire.fields.contains_key("result");
        let has_error = wire.fields.contains_key("error");
        match (wire.ok, has_result, has_error) {
            (true, true, false) => Ok(Self::Success(
                wire.fields
                    .remove("result")
                    .expect("result field was checked"),
            )),
            (false, false, true) => serde_json::from_value::<FinalError>(
                wire.fields
                    .remove("error")
                    .expect("error field was checked"),
            )
            .map(Self::Error)
            .map_err(de::Error::custom),
            _ => Err(de::Error::custom("malformed final_result")),
        }
    }
}

impl Serialize for FinalResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Success(result) => {
                let mut state = serializer.serialize_struct("final_result", 2)?;
                state.serialize_field("ok", &true)?;
                state.serialize_field("result", result)?;
                state.end()
            }
            Self::Error(error) => {
                let mut state = serializer.serialize_struct("final_result", 2)?;
                state.serialize_field("ok", &false)?;
                state.serialize_field("error", error)?;
                state.end()
            }
        }
    }
}

/// Write one JSON Lines protocol message to the parent.
fn write_protocol_line<T: Serialize>(value: &T) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, value).map_err(std::io::Error::other)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn append_protocol_chunk(line: &mut Vec<u8>, chunk: &[u8], limit: usize) -> std::io::Result<()> {
    if line.len().saturating_add(chunk.len()) > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "protocol line exceeded the size limit",
        ));
    }
    line.extend_from_slice(chunk);
    Ok(())
}

/// Read one bounded protocol line. `None` is EOF before any line.
fn read_limited_line_with_limit(
    reader: &mut impl BufRead,
    limit: usize,
) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let (take, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        append_protocol_chunk(&mut line, &available[..take], limit)?;
        reader.consume(take);
        if complete {
            break;
        }
    }
    if line.is_empty() {
        return Ok(None);
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_limited_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    read_limited_line_with_limit(reader, MAX_PROTOCOL_LINE_BYTES)
}

/// Read one protocol line from the parent. `None` is EOF before any line.
fn read_protocol_line() -> std::io::Result<Option<String>> {
    read_limited_line(&mut std::io::stdin().lock())
}

/// Send one host call and wait for the parent's exactly-one result.
fn worker_host_call(name: BridgeName, payload: &Json) -> boa_engine::JsResult<Json> {
    let call = WorkerMessage::HostCall {
        name,
        payload: payload.clone(),
    };
    write_protocol_line(&call).map_err(|error| {
        JsNativeError::error()
            .with_message(format!("{}: cannot reach host: {error}", name.as_str()))
    })?;
    let line = read_protocol_line().map_err(|error| {
        JsNativeError::error().with_message(format!("{}: host read failed: {error}", name.as_str()))
    })?;
    let line = line.ok_or_else(|| {
        JsNativeError::error().with_message(format!("{}: host closed the pipe", name.as_str()))
    })?;
    let message: ParentMessage = serde_json::from_str(&line).map_err(|error| {
        JsNativeError::error()
            .with_message(format!("{}: invalid host response: {error}", name.as_str()))
    })?;
    drop(line);
    match message {
        ParentMessage::HostResult { result } => Ok(result),
        ParentMessage::Job { .. } => Err(JsNativeError::error()
            .with_message(format!("{}: expected host_result", name.as_str()))
            .into()),
    }
}

/// Decode one bridge argument, dispatch it to the parent, and encode the JSON
/// string the script prelude expects.
fn worker_bridge(name: BridgeName, args: &[JsValue]) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message(format!("{}: expected a JSON string", name.as_str()))
            .into());
    };
    let payload: Json = serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
        JsNativeError::typ().with_message(format!("{}: invalid payload: {error}", name.as_str()))
    })?;
    let result = worker_host_call(name, &payload)?;
    let rendered = serde_json::to_string(&result).map_err(|error| {
        JsNativeError::error()
            .with_message(format!("{}: cannot encode result: {error}", name.as_str()))
    })?;
    drop(result);
    Ok(JsValue::from(JsString::from(rendered)))
}

fn worker_http_get(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::HttpGet, args)
}

fn worker_http_pipe(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::HttpPipe, args)
}

fn worker_file(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::File, args)
}

fn worker_upload(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::Upload, args)
}

fn worker_state(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::State, args)
}

fn worker_validate_headers(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::ValidateHeaders, args)
}

fn worker_crypto(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge(BridgeName::Crypto, args)
}

/// Validate script-supplied response headers with the HTTP grammar shared by
/// the final response path.
pub(crate) fn validated_header_pairs(
    pairs: &[(String, String)],
) -> Result<Vec<(HeaderName, HeaderValue)>, String> {
    pairs
        .iter()
        .map(|(name, value)| {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("invalid header name {name:?}"))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|_| format!("invalid value for header {name:?}"))?;
            Ok((header_name, header_value))
        })
        .collect()
}

/// Process environment snapshot exposed as `ctx.env`.
/// tradeoff: read per request and never persisted; no `.env` file in v1.
fn env_json() -> Json {
    let mut map = serde_json::Map::new();
    for (key, value) in std::env::vars() {
        // Internal worker tuning, not part of the script-visible environment.
        if key == MALLOC_ARENA_MAX_ENV {
            continue;
        }
        map.insert(key, Json::String(value));
    }
    Json::Object(map)
}

/// Split a raw query string into decoded key/value pairs.
#[must_use]
fn parse_query(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decode(key.replace('+', " ").as_bytes()),
                percent_decode(value.replace('+', " ").as_bytes()),
            )
        })
        .collect()
}

/// Contain an engine panic: it becomes a script failure instead of unwinding
/// into the server task. A named function so the guard itself is testable.
fn guard_engine<T>(run: impl FnOnce() -> T) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(run))
        .map_err(|_| Error::Failed("script panicked in the engine".into()))
}

/// Fill the prelude's trusted, compile-time placeholders.
fn prelude() -> String {
    PRELUDE
        .replace("__SD_API_VERSION__", API_VERSION)
        .replace("__SD_STATE_MAX_TTL_MS__", &state::MAX_TTL_MS.to_string())
        .replace(
            "__SD_STATE_MAX_KEY_BYTES__",
            &state::MAX_KEY_BYTES.to_string(),
        )
        .replace(
            "__SD_STATE_MAX_DEPTH__",
            &state::MAX_VALUE_DEPTH.to_string(),
        )
}

fn register_worker_bridges(context: &mut Context) -> Result<(), Error> {
    for (name, func) in [
        (
            js_string!("__sd_http_get"),
            NativeFunction::from_fn_ptr(worker_http_get),
        ),
        (
            js_string!("__sd_http_pipe"),
            NativeFunction::from_fn_ptr(worker_http_pipe),
        ),
        (
            js_string!("__sd_validate_headers"),
            NativeFunction::from_fn_ptr(worker_validate_headers),
        ),
        (
            js_string!("__sd_file"),
            NativeFunction::from_fn_ptr(worker_file),
        ),
        (
            js_string!("__sd_upload"),
            NativeFunction::from_fn_ptr(worker_upload),
        ),
        (
            js_string!("__sd_state"),
            NativeFunction::from_fn_ptr(worker_state),
        ),
        (
            js_string!("__sd_crypto"),
            NativeFunction::from_fn_ptr(worker_crypto),
        ),
    ] {
        context
            .register_global_builtin_callable(name, 1, func)
            .map_err(|error| Error::Failed(error.to_string()))?;
    }
    Ok(())
}

/// Evaluate one script inside the worker process. Every host function is
/// forwarded to the parent; upstream, file, and upload policy stay there.
fn evaluate_in_worker(source: &str, request: &Json) -> Result<Json, Error> {
    let upstream_marker = upstream_marker();
    let prelude = prelude();
    let mut context = Context::default();
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);
    limits.set_stack_size_limit(VM_STACK_SIZE_LIMIT);
    // A Rust-side panic in the engine must not take the worker down without a
    // protocol result; it is mapped to the same script failure as before.
    let staged = guard_engine(|| -> Result<String, Error> {
        let request = JsValue::from_json(request, &mut context)
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_property(js_string!("__sd_request"), request, Attribute::all())
            .map_err(|error| Error::Failed(error.to_string()))?;
        let env = JsValue::from_json(&env_json(), &mut context)
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_property(js_string!("__sd_env"), env, Attribute::all())
            .map_err(|error| Error::Failed(error.to_string()))?;
        let marker = JsValue::from_json(&Json::String(upstream_marker.clone()), &mut context)
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_property(js_string!("__sd_upstream_marker"), marker, Attribute::all())
            .map_err(|error| Error::Failed(error.to_string()))?;
        register_worker_bridges(&mut context)?;
        let evaluated = (|| -> Result<String, JsError> {
            context.eval(Source::from_bytes(&prelude))?;
            context.eval(Source::from_bytes(source))?;
            let dump = context.eval(Source::from_bytes(EXTRACT))?;
            Ok(match dump.as_string() {
                Some(text) => text.to_std_string_escaped(),
                None => "null".to_string(),
            })
        })();
        match evaluated {
            Ok(dump) => Ok(dump),
            Err(error) => match upstream_unreachable_kind(&error, &upstream_marker, &mut context) {
                Some(kind) => Err(Error::UpstreamUnreachable {
                    message: error.to_string(),
                    kind,
                }),
                // The Job Object commit cap fails allocations inside Boa on
                // Windows, so the engine-level detector is not Linux-only.
                None if cfg!(any(target_os = "linux", target_os = "windows"))
                    && engine_out_of_memory(&error) =>
                {
                    Err(Error::MemoryLimitExceeded)
                }
                None => Err(Error::Failed(error.to_string())),
            },
        }
    });
    let dump = match staged {
        Err(error) | Ok(Err(error)) => return Err(error),
        Ok(Ok(dump)) => dump,
    };
    serde_json::from_str(&dump)
        .map_err(|error| Error::Failed(format!("cannot read script state: {error}")))
}

/// Boa 0.22 maps allocator failure from `AlignedVec` to this native
/// `RangeError`; capacity overflow uses a different message and is a script
/// error, not a configured memory-limit kill.
fn engine_out_of_memory(error: &JsError) -> bool {
    error.as_native().is_some_and(|native| {
        matches!(native.kind(), JsNativeErrorKind::Range)
            && native.message().starts_with("invalid layout ")
            && native.message().ends_with(" while allocating data block")
    })
}

fn is_allocator_abort_line(line: &str) -> bool {
    line.starts_with("memory allocation of ") && line.ends_with(" failed")
}

/// Per-request marker property for host-created transport errors. It is not a
/// security token; it prevents an unrelated script throw from being classified
/// as an upstream failure merely by copying the documented `error.code`.
fn upstream_marker() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    hasher.write_u64(sequence);
    format!("__sd_upstream_{:016x}", hasher.finish())
}

/// Recognize an uncaught error created by the `ctx.http` transport path and
/// recover its stable kind. The script-visible `error.code` is deliberately
/// not trusted on its own.
fn upstream_unreachable_kind(
    error: &JsError,
    marker: &str,
    context: &mut Context,
) -> Option<&'static str> {
    let object = error.as_opaque().and_then(JsValue::as_object)?;
    let key = JsString::from(marker);
    if !object
        .has_own_property(key.clone(), context)
        .unwrap_or(false)
    {
        return None;
    }
    let kind = object
        .get(key, context)
        .ok()
        .and_then(|value| value.as_string())
        .map(|value| value.to_std_string_escaped());
    Some(match kind.as_deref() {
        Some("timeout") => "timeout",
        Some("dns") => "dns",
        _ => "transport",
    })
}

/// Turn the extracted JSON record into an `Outcome`. A streamed response
/// carries only its status and headers through the realm; the body stream is
/// handed back separately by the host bridge.
fn parse_host_record(
    host: &Json,
    pipe_stream: Option<upstream::PipeBody>,
    file_host: Option<files::FileAccess>,
    upload_host: Option<files::UploadAccess>,
) -> Outcome {
    let logs = parse_script_logs(host);
    let Some(response) = host.get("response").filter(|value| !value.is_null()) else {
        return script_outcome(None, logs, Some(Error::NoResponse));
    };
    let Some(status) = response
        .get("status")
        .and_then(Json::as_u64)
        .and_then(|value| u16::try_from(value).ok())
    else {
        return script_outcome(
            None,
            logs,
            Some(Error::Failed("ctx.respond: status was not recorded".into())),
        );
    };
    let headers = parse_script_headers(response);
    let body = match parse_script_body(response, pipe_stream, file_host, upload_host) {
        Ok(body) => body,
        Err(error) => return script_outcome(None, logs, Some(error)),
    };
    script_outcome(
        Some(ScriptResponse {
            status,
            headers,
            body,
        }),
        logs,
        None,
    )
}

/// Assemble one script run result; `execute` finalizes the shared call chains.
fn script_outcome(
    response: Option<ScriptResponse>,
    logs: Vec<LogRecord>,
    error: Option<Error>,
) -> Outcome {
    Outcome {
        response,
        logs,
        error,
        upstream_calls: Vec::new(),
        file_calls: files::CallLog::default(),
    }
}

fn parse_script_logs(host: &Json) -> Vec<LogRecord> {
    host.get("logs")
        .and_then(Json::as_array)
        .map_or_else(Vec::new, |entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(LogRecord {
                        level: entry.get("level")?.as_str()?.to_string(),
                        message: entry.get("message")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
}

fn parse_script_headers(response: &Json) -> Vec<(String, String)> {
    response
        .get("headers")
        .and_then(Json::as_array)
        .map_or_else(Vec::new, |rows| {
            rows.iter()
                .filter_map(|row| {
                    let pair = row.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
}

/// Resolve the recorded body: buffered text/bytes, or the host-side stream
/// handed back for `ctx.http.pipe` / `ctx.file.stream`.
fn parse_script_body(
    response: &Json,
    pipe_stream: Option<upstream::PipeBody>,
    file_host: Option<files::FileAccess>,
    upload_host: Option<files::UploadAccess>,
) -> Result<ResponseBody, Error> {
    if response.get("stream").and_then(Json::as_bool) == Some(true) {
        let Some(stream) = pipe_stream else {
            return Err(Error::Failed(
                "ctx.http.pipe: streamed response was not recorded".into(),
            ));
        };
        return Ok(ResponseBody::Stream(stream));
    }
    if let Some(handle) = response
        .get("body")
        .and_then(|body| body.get("file"))
        .and_then(Json::as_u64)
    {
        let Some(host) = file_host else {
            return Err(Error::Failed(
                "ctx.file.stream: file access was not recorded".into(),
            ));
        };
        let Some(file) = host.take_stream(handle) else {
            return Err(Error::Failed(
                "ctx.file.stream: handle was not recorded".into(),
            ));
        };
        return Ok(ResponseBody::File(file));
    }
    if let Some(handle) = response
        .get("body")
        .and_then(|body| body.get("upload"))
        .and_then(Json::as_u64)
    {
        let Some(host) = upload_host else {
            return Err(Error::Failed(
                "ctx.request.files: upload access was not recorded".into(),
            ));
        };
        let Some(file) = host.take_stream(handle) else {
            return Err(Error::Failed(
                "ctx.request.files: upload stream was not recorded".into(),
            ));
        };
        return Ok(ResponseBody::File(file));
    }
    Ok(match response.get("body") {
        Some(Json::String(text)) => ResponseBody::Text(text.clone()),
        Some(Json::Array(bytes)) => ResponseBody::Bytes(
            bytes
                .iter()
                .filter_map(Json::as_u64)
                .filter_map(|value| u8::try_from(value).ok())
                .collect(),
        ),
        _ => ResponseBody::Text(String::new()),
    })
}

/// Parent-owned host capabilities and parent-held streams for one run.
struct HostState {
    upstream: Option<upstream::UpstreamAccess>,
    files: Option<files::FileAccess>,
    uploads: Option<files::UploadAccess>,
    pipe: Option<upstream::PipeBody>,
    shared_state: SharedState,
}

struct HostStateInit<'a> {
    upstream_config: &'a UpstreamConfig,
    files_config: &'a FilesConfig,
    uploads: Option<Arc<files::UploadStore>>,
    script_deadline: Instant,
    client_range: Option<String>,
    calls: upstream::CallLog,
    file_calls: files::CallLog,
    shared_state: SharedState,
}

impl HostState {
    fn new(init: HostStateInit<'_>) -> Self {
        let HostStateInit {
            upstream_config,
            files_config,
            uploads,
            script_deadline,
            client_range,
            calls,
            file_calls,
            shared_state,
        } = init;
        let upstream = upstream::UpstreamAccess::new(
            upstream_config.allow_hosts.clone(),
            Duration::from_millis(upstream_config.timeout_ms),
            script_deadline,
            client_range,
            calls,
        );
        let files = files::FileAccess::new(&files_config.root, file_calls.clone());
        let uploads = uploads.map(|store| files::UploadAccess::new(store, file_calls));
        Self {
            upstream: Some(upstream),
            files: Some(files),
            uploads,
            pipe: None,
            shared_state,
        }
    }

    /// Handle one bridge call. `None` is a missing configured capability, which
    /// is a parent/worker protocol mismatch rather than a script error.
    fn dispatch(&mut self, name: BridgeName, payload: &Json) -> Option<Json> {
        match name {
            BridgeName::HttpGet => {
                let host = self.upstream.as_ref()?;
                Some(match host.get(payload) {
                    Ok(response) => {
                        json!({ "ok": true, "response": upstream::response_json(response) })
                    }
                    Err(error) => upstream_error_json(&error),
                })
            }
            BridgeName::HttpPipe => {
                let host = self.upstream.as_ref()?;
                Some(match host.pipe(payload) {
                    Ok(response) => {
                        self.pipe = Some(response.body);
                        json!({
                            "ok": true,
                            "status": response.status,
                            "headers": response
                                .headers
                                .iter()
                                .map(|(name, value)| json!([name, value]))
                                .collect::<Vec<_>>(),
                        })
                    }
                    Err(error) => upstream_error_json(&error),
                })
            }
            BridgeName::File => Some(self.files.as_ref()?.call(payload)),
            BridgeName::Upload => Some(self.uploads.as_ref().map_or_else(
                || {
                    json!({
                        "ok": false,
                        "code": "script_error",
                        "message": "__sd_upload called outside a script request"
                    })
                },
                |host| host.call(payload),
            )),
            BridgeName::State => Some(dispatch_state(&self.shared_state, payload)),
            BridgeName::ValidateHeaders => Some(validate_headers_json(payload)),
            BridgeName::Crypto => Some(crypto::dispatch_crypto(payload)),
        }
    }

    /// Move parent-held streams into the final response only after a complete
    /// worker result. On any earlier exit they are dropped with the host.
    fn take_streams(
        &mut self,
    ) -> (
        Option<upstream::PipeBody>,
        Option<files::FileAccess>,
        Option<files::UploadAccess>,
    ) {
        (self.pipe.take(), self.files.take(), self.uploads.take())
    }
}

/// Stable JSON shape for an upstream bridge failure.
fn upstream_error_json(error: &upstream::Error) -> Json {
    json!({
        "ok": false,
        "code": error.code(),
        "kind": error.transport_kind(),
        "message": error.message(),
    })
}

fn state_error_json(error: state::StateError) -> Json {
    let code = error.code();
    json!({
        "ok": false,
        "code": code,
        "message": code,
    })
}

fn parse_state_ttl(payload: &Json) -> Result<Option<Duration>, state::StateError> {
    let Some(ttl_ms) = payload.get("ttl_ms") else {
        return Ok(None);
    };
    let ttl_ms = ttl_ms
        .as_u64()
        .filter(|ttl_ms| *ttl_ms > 0 && *ttl_ms <= state::MAX_TTL_MS)
        .ok_or(state::StateError::InvalidInput)?;
    Ok(Some(Duration::from_millis(ttl_ms)))
}

fn dispatch_state(shared_state: &SharedState, payload: &Json) -> Json {
    let Some(operation) = payload.get("op").and_then(Json::as_str) else {
        return state_error_json(state::StateError::InvalidInput);
    };
    let Some(key) = payload.get("key").and_then(Json::as_str) else {
        return state_error_json(state::StateError::InvalidInput);
    };
    let ttl = match parse_state_ttl(payload) {
        Ok(ttl) => ttl,
        Err(error) => return state_error_json(error),
    };
    let Ok(mut store) = shared_state.lock() else {
        return state_error_json(state::StateError::Internal);
    };
    match operation {
        "get" => match store.get(key) {
            Ok(Some(value)) => json!({ "ok": true, "found": true, "value": value }),
            Ok(None) => json!({ "ok": true, "found": false }),
            Err(error) => state_error_json(error),
        },
        "set" => {
            let Some(value) = payload.get("value").cloned() else {
                return state_error_json(state::StateError::InvalidInput);
            };
            match store.set(key, value, ttl) {
                Ok(()) => json!({ "ok": true }),
                Err(error) => state_error_json(error),
            }
        }
        "take" => match store.take(key) {
            Ok(Some(value)) => json!({ "ok": true, "found": true, "value": value }),
            Ok(None) => json!({ "ok": true, "found": false }),
            Err(error) => state_error_json(error),
        },
        "setIfAbsent" => {
            let Some(value) = payload.get("value").cloned() else {
                return state_error_json(state::StateError::InvalidInput);
            };
            match store.set_if_absent(key, value, ttl) {
                Ok(stored) => json!({ "ok": true, "stored": stored }),
                Err(error) => state_error_json(error),
            }
        }
        _ => state_error_json(state::StateError::InvalidInput),
    }
}

/// Parent-side header validation bridge; the worker never owns this policy.
fn validate_headers_json(payload: &Json) -> Json {
    match serde_json::from_value::<Vec<(String, String)>>(payload.clone()) {
        Ok(pairs) => match validated_header_pairs(&pairs) {
            Ok(_) => json!({ "ok": true }),
            Err(message) => json!({
                "ok": false,
                "code": "script_error",
                "message": message,
            }),
        },
        Err(_) => json!({
            "ok": false,
            "code": "script_error",
            "message": "__sd_validate_headers: invalid payload",
        }),
    }
}

/// Run one blocking host call without stalling the async protocol loop. Calls
/// are strictly serialized by ping-pong, so one mutex is sufficient.
async fn dispatch_host(
    host: &Arc<Mutex<HostState>>,
    name: BridgeName,
    payload: Json,
) -> Option<Json> {
    let host = Arc::clone(host);
    tokio::task::spawn_blocking(move || {
        let mut host = host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        host.dispatch(name, &payload)
    })
    .await
    .ok()
    .flatten()
}

/// Write one JSON Lines message to the worker.
async fn write_json_line<T: Serialize>(
    writer: &mut tokio::process::ChildStdin,
    value: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

async fn read_limited_line_async_with_limit<R>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        let (take, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        append_protocol_chunk(&mut line, &available[..take], limit)?;
        reader.consume(take);
        if complete {
            break;
        }
    }
    if line.is_empty() {
        return Ok(None);
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

async fn read_limited_line_async<R>(reader: &mut R) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    read_limited_line_async_with_limit(reader, MAX_PROTOCOL_LINE_BYTES).await
}

enum WorkerReply {
    /// The worker delivered its one complete final result.
    Final(FinalResult),
    /// The wall-clock deadline expired.
    TimedOut,
    /// EOF, malformed output, an unknown bridge, or another protocol error.
    Unexpected,
}

/// Pump the strict request/response protocol until `final_result`.
async fn exchange(
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    host: &Arc<Mutex<HostState>>,
    job: &ParentMessage,
    deadline: tokio::time::Instant,
) -> WorkerReply {
    if write_json_line(&mut stdin, job).await.is_err() {
        return WorkerReply::Unexpected;
    }
    let mut lines = BufReader::new(stdout);
    let mut final_result = None;
    loop {
        let line =
            match tokio::time::timeout_at(deadline, read_limited_line_async(&mut lines)).await {
                Err(_) => return WorkerReply::TimedOut,
                Ok(Err(_)) => return WorkerReply::Unexpected,
                Ok(Ok(line)) => line,
            };
        let Some(line) = line else {
            return final_result.map_or(WorkerReply::Unexpected, WorkerReply::Final);
        };
        // `final_result` is the last protocol line; any trailing stdout is a
        // protocol error even when the extra line is valid JSON.
        if final_result.is_some() {
            return WorkerReply::Unexpected;
        }
        let Ok(message) = serde_json::from_str::<WorkerMessage>(&line) else {
            return WorkerReply::Unexpected;
        };
        match message {
            WorkerMessage::HostCall { name, payload } => {
                let result =
                    match tokio::time::timeout_at(deadline, dispatch_host(host, name, payload))
                        .await
                    {
                        Err(_) => return WorkerReply::TimedOut,
                        Ok(None) => return WorkerReply::Unexpected,
                        Ok(Some(result)) => result,
                    };
                let reply = ParentMessage::HostResult { result };
                match tokio::time::timeout_at(deadline, write_json_line(&mut stdin, &reply)).await {
                    Err(_) => return WorkerReply::TimedOut,
                    Ok(Err(_)) => return WorkerReply::Unexpected,
                    Ok(Ok(())) => {}
                }
            }
            WorkerMessage::FinalResult(result) => final_result = Some(result),
        }
    }
}

#[derive(Default)]
struct WorkerDiagnostics {
    memory_allocation_failed: bool,
}

/// Drain worker stderr to the operator log. It never reaches the client, and a
/// full pipe cannot block the worker because this task keeps reading.
async fn forward_worker_stderr(stderr: tokio::process::ChildStderr) -> WorkerDiagnostics {
    let mut diagnostics = WorkerDiagnostics::default();
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if is_allocator_abort_line(&line) {
            diagnostics.memory_allocation_failed = true;
        }
        eprintln!("{line}");
    }
    diagnostics
}

fn unexpected_worker_error(diagnostics: &WorkerDiagnostics) -> Error {
    // Both platforms enforce a process memory bound, so the allocator abort
    // line is a memory-limit signal on either one.
    if cfg!(any(target_os = "linux", target_os = "windows")) && diagnostics.memory_allocation_failed
    {
        Error::MemoryLimitExceeded
    } else {
        Error::WorkerTerminated
    }
}

/// Spawn one worker process and drive it until its final result or the shared
/// wall-clock deadline. The child is killed and reaped on every other exit.
async fn run_worker(
    job: ParentMessage,
    memory_limit_mb: u64,
    host: Arc<Mutex<HostState>>,
    deadline: tokio::time::Instant,
) -> Outcome {
    let Ok(executable) = std::env::current_exe() else {
        return Outcome::failed(Error::WorkerTerminated);
    };
    let mut command = Command::new(executable);
    command
        .arg(WORKER_SUBCOMMAND)
        .arg("--memory-limit-mb")
        .arg(memory_limit_mb.to_string())
        // The sentinel adds a thread. Without this, glibc reserves a 64 MiB
        // malloc arena for it, a large share of the script budget; forcing one
        // arena keeps the process-wide bound meaningful.
        .env(MALLOC_ARENA_MAX_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // `spawn` can block inside fork/exec; run it on the blocking pool so the
    // request deadline stays the outer bound. A child arriving after the
    // timeout is still killed because `kill_on_drop` stays set on the command.
    let mut child = match tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || command.spawn()),
    )
    .await
    {
        Err(_) => return Outcome::failed(Error::TimedOut),
        // Inner `Err` is a join error; `Ok(Err)` is a failed `Command::spawn`.
        Ok(Err(_) | Ok(Err(_))) => return Outcome::failed(Error::WorkerTerminated),
        Ok(Ok(Ok(child))) => child,
    };
    // ADR 0014: Windows has no in-process bound, so the parent puts the worker
    // into its own Job Object before the worker can read the job line. This is
    // fail-closed like a failing Linux `setrlimit`: without the job the worker
    // would run unbounded, so refuse to use it. The guard stays alive (and so
    // keeps the job open) until this function drops it after the worker ended.
    #[cfg(windows)]
    let _worker_job = match assign_worker_job(&child, memory_limit_mb) {
        Ok(worker_job) => worker_job,
        Err(error) => {
            // Operator-facing only; the client still sees a generic script
            // error and never an internal diagnostic.
            eprintln!("script worker job object failed: {error}");
            terminate(&mut child).await;
            return Outcome::failed(Error::WorkerTerminated);
        }
    };
    let Some(stdin) = child.stdin.take() else {
        terminate(&mut child).await;
        return Outcome::failed(Error::WorkerTerminated);
    };
    let Some(stdout) = child.stdout.take() else {
        terminate(&mut child).await;
        return Outcome::failed(Error::WorkerTerminated);
    };
    let Some(stderr) = child.stderr.take() else {
        terminate(&mut child).await;
        return Outcome::failed(Error::WorkerTerminated);
    };
    let stderr_task = tokio::spawn(forward_worker_stderr(stderr));
    let reply = {
        let exchange = exchange(stdin, stdout, &host, &job, deadline);
        tokio::pin!(exchange);
        tokio::select! {
            biased;
            reply = &mut exchange => reply,
            () = tokio::time::sleep_until(deadline) => WorkerReply::TimedOut,
        }
    };
    match reply {
        WorkerReply::TimedOut => {
            terminate(&mut child).await;
            let _ = stderr_task.await;
            Outcome::failed(Error::TimedOut)
        }
        WorkerReply::Unexpected => {
            terminate(&mut child).await;
            let diagnostics = stderr_task.await.unwrap_or_default();
            Outcome::failed(unexpected_worker_error(&diagnostics))
        }
        WorkerReply::Final(result) => {
            // The deadline still covers process teardown. A complete result is
            // consumed only after a successful exit; a lingering worker is
            // killed and reported as a timeout, and parent-held streams drop.
            match tokio::time::timeout_at(deadline, child.wait()).await {
                Ok(Ok(status)) if status.success() => {
                    let _ = stderr_task.await;
                    finish_outcome(result, &host)
                }
                Ok(Ok(_)) => {
                    let diagnostics = stderr_task.await.unwrap_or_default();
                    Outcome::failed(unexpected_worker_error(&diagnostics))
                }
                Ok(Err(_)) => {
                    terminate(&mut child).await;
                    let _ = stderr_task.await;
                    Outcome::failed(Error::WorkerTerminated)
                }
                Err(_) => {
                    terminate(&mut child).await;
                    let _ = stderr_task.await;
                    Outcome::failed(Error::TimedOut)
                }
            }
        }
    }
}

/// Kill and reap a worker that did not reach a normal final result.
async fn terminate(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Put a freshly spawned worker into its own Job Object (Windows only).
///
/// The cap is the same `sandbox.script_memory_limit_mb` value Linux applies as
/// `RLIMIT_AS`; on Windows it bounds commit charge, and kill-on-close makes a
/// hard-killed server take its workers down with it.
#[cfg(windows)]
fn assign_worker_job(
    child: &Child,
    memory_limit_mb: u64,
) -> std::io::Result<crate::windows_job::WorkerJob> {
    let bytes = memory_limit_mb
        .checked_mul(1024 * 1024)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "memory limit too large")
        })?;
    let process = child
        .raw_handle()
        .ok_or_else(|| std::io::Error::other("worker exited before its job was assigned"))?;
    let worker_job = crate::windows_job::WorkerJob::create(bytes)?;
    worker_job.assign(process)?;
    Ok(worker_job)
}

/// Map one complete `final_result` onto the existing Outcome shape.
fn finish_outcome(result: FinalResult, host: &Arc<Mutex<HostState>>) -> Outcome {
    let (pipe, file_host, upload_host) = {
        let mut host = host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        host.take_streams()
    };
    match result {
        FinalResult::Success(result) => parse_host_record(&result, pipe, file_host, upload_host),
        FinalResult::Error(error) => Outcome::failed(error.into_error()),
    }
}

/// Run one script for one request in a fresh worker process.
///
/// Must use result: a dead or unreachable worker is reported, never a silent
/// success. The semaphore permit is held for the whole worker lifetime. This
/// standalone entry point starts a fresh process-local State store; `serve`
/// uses one store shared across its requests.
pub async fn execute(
    source: String,
    request: RequestSnapshot,
    sandbox: SandboxConfig,
    upstream: UpstreamConfig,
    files_config: FilesConfig,
    uploads: Option<Arc<files::UploadStore>>,
    worker_slot: tokio::sync::OwnedSemaphorePermit,
) -> Outcome {
    execute_with_state(
        source,
        request,
        ExecutionContext {
            sandbox,
            upstream,
            files_config,
            uploads,
            shared_state: state::new_shared_state(),
            worker_slot,
        },
    )
    .await
}

pub(crate) struct ExecutionContext {
    pub(crate) sandbox: SandboxConfig,
    pub(crate) upstream: UpstreamConfig,
    pub(crate) files_config: FilesConfig,
    pub(crate) uploads: Option<Arc<files::UploadStore>>,
    pub(crate) shared_state: SharedState,
    pub(crate) worker_slot: tokio::sync::OwnedSemaphorePermit,
}

pub(crate) async fn execute_with_state(
    source: String,
    request: RequestSnapshot,
    context: ExecutionContext,
) -> Outcome {
    let ExecutionContext {
        sandbox,
        upstream,
        files_config,
        uploads,
        shared_state,
        worker_slot,
    } = context;
    let _slot = worker_slot;
    let timeout = Duration::from_millis(sandbox.script_timeout_ms);
    let script_deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let deadline = tokio::time::Instant::from_std(script_deadline);
    let calls: upstream::CallLog = upstream::CallLog::new();
    let file_calls: files::CallLog = files::CallLog::new();
    let client_range = request
        .headers
        .iter()
        .find(|(name, _)| name == "range")
        .map(|(_, value)| value.clone());
    let host = Arc::new(Mutex::new(HostState::new(HostStateInit {
        upstream_config: &upstream,
        files_config: &files_config,
        uploads: uploads.clone(),
        script_deadline,
        client_range,
        calls: calls.clone(),
        file_calls: file_calls.clone(),
        shared_state,
    })));
    let job = ParentMessage::Job {
        script: source,
        request: request.to_json(),
    };
    let mut outcome = run_worker(
        job,
        sandbox.script_memory_limit_mb,
        Arc::clone(&host),
        deadline,
    )
    .await;
    if outcome.error.is_some() {
        // A failed run can leave a blocking host call holding the store; close
        // its contents now so temporary uploads do not leak on timeout or a
        // worker crash.
        if let Some(store) = &uploads {
            store.close();
        }
    }
    outcome.upstream_calls = calls.snapshot();
    outcome.file_calls = file_calls;
    outcome
}

/// Hidden entry point run by a self-spawned script worker.
#[must_use]
pub fn run_worker_process(memory_limit_mb: u64) -> i32 {
    match worker_main(memory_limit_mb) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("script worker: {error}");
            1
        }
    }
}

/// Exit this worker when its parent disappears.
///
/// On Unix a killed server leaves its workers reparented and still running.
/// The sentinel polls `parent_id()` once a second, independently of the script
/// thread, so a runaway script cannot suppress the cleanup. Its stack is kept
/// small because it counts against the worker's configured memory budget.
#[cfg(unix)]
fn spawn_parent_sentinel() -> std::io::Result<()> {
    use std::os::unix::process::parent_id;
    use std::sync::mpsc;

    let parent = parent_id();
    let (started, ready) = mpsc::sync_channel(0);
    std::thread::Builder::new()
        .name("script-worker-sentinel".into())
        .stack_size(SENTINEL_STACK_SIZE)
        .spawn(move || {
            // Rust installs this thread's alternate signal stack before the
            // closure runs. Report readiness so RLIMIT_AS is applied only
            // after the sentinel is fully initialized.
            let _ = started.send(());
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if parent_id() != parent {
                    std::process::exit(0);
                }
            }
        })
        .map(|_| ())?;
    ready
        .recv()
        .map_err(|_| std::io::Error::other("script worker sentinel failed to start"))
}

#[cfg(not(unix))]
fn spawn_parent_sentinel() -> std::io::Result<()> {
    Ok(())
}

fn worker_main(memory_limit_mb: u64) -> std::io::Result<()> {
    if memory_limit_mb < SCRIPT_MEMORY_LIMIT_FLOOR_MB {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory limit is below the compiled-in floor",
        ));
    }
    // ADR 0014 D9: create the sentinel before applying the memory bound. The
    // worker environment sets MALLOC_ARENA_MAX=1, so the thread costs only its
    // small stack instead of a 64 MiB glibc arena.
    spawn_parent_sentinel()?;
    // Linux applies its bound here, before reading the job, because parsing a
    // large script or request snapshot is itself a meaningful allocation.
    // Windows is bounded from the parent's side: its Job Object is already
    // assigned before this process can read the job line. macOS keeps process
    // isolation and deadline kill without claiming a hard memory bound.
    apply_worker_memory_limit(memory_limit_mb)?;
    let Some(line) = read_protocol_line()? else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "missing job line",
        ));
    };
    let message: ParentMessage = serde_json::from_str(&line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let ParentMessage::Job { script, request } = message else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected a job line",
        ));
    };
    // Boa recurses on the native stack; run it on a thread with a fixed,
    // platform-independent stack so Windows' 1 MiB main-thread default cannot
    // turn an ordinary script into a stack-overflow crash.
    std::thread::Builder::new()
        .name("script-worker-eval".into())
        .stack_size(SCRIPT_THREAD_STACK_SIZE)
        .spawn(move || run_job(&script, &request))
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot start script evaluation thread: {error}"),
            )
        })?
        .join()
        .map_err(|_| std::io::Error::other("script evaluation thread panicked"))?
}

fn run_job(script: &str, request: &Json) -> std::io::Result<()> {
    let message = match evaluate_in_worker(script, request) {
        Ok(result) => WorkerMessage::FinalResult(FinalResult::Success(result)),
        Err(Error::UpstreamUnreachable { message, kind }) => {
            let Some(kind) = FinalErrorKind::from_upstream(kind) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unknown upstream error kind",
                ));
            };
            WorkerMessage::FinalResult(FinalResult::Error(FinalError {
                kind: Some(kind),
                message,
            }))
        }
        Err(Error::Failed(message)) => WorkerMessage::FinalResult(FinalResult::Error(FinalError {
            kind: None,
            message,
        })),
        Err(Error::MemoryLimitExceeded) => {
            WorkerMessage::FinalResult(FinalResult::Error(FinalError {
                kind: Some(FinalErrorKind::MemoryLimit),
                message: Error::MemoryLimitExceeded.detail().to_string(),
            }))
        }
        Err(error) => WorkerMessage::FinalResult(FinalResult::Error(FinalError {
            kind: None,
            message: error.detail().to_string(),
        })),
    };
    write_protocol_line(&message)
}

#[cfg(target_os = "linux")]
fn apply_worker_memory_limit(memory_limit_mb: u64) -> std::io::Result<()> {
    use rlimit::{setrlimit, Resource};

    let bytes = memory_limit_mb.checked_mul(1024 * 1024).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "memory limit too large")
    })?;
    setrlimit(Resource::AS, bytes, bytes)
}

#[cfg(not(target_os = "linux"))]
fn apply_worker_memory_limit(_memory_limit_mb: u64) -> std::io::Result<()> {
    // Windows is already bounded by the parent-assigned Job Object, and macOS
    // has no process memory bound; both parents check the floor before spawn.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_protocol_round_trips_host_calls_and_results() {
        let host_call = WorkerMessage::HostCall {
            name: BridgeName::HttpGet,
            payload: json!({ "url": "http://example.test" }),
        };
        assert_round_trip(
            &host_call,
            &json!({
                "type": "host_call",
                "name": "__sd_http_get",
                "payload": { "url": "http://example.test" }
            }),
        );

        let host_result = ParentMessage::HostResult {
            result: json!({ "ok": true }),
        };
        assert_round_trip(
            &host_result,
            &json!({ "type": "host_result", "result": { "ok": true } }),
        );
    }

    #[test]
    fn bridge_names_use_stable_wire_labels() {
        for (name, label) in [
            (BridgeName::HttpGet, "__sd_http_get"),
            (BridgeName::HttpPipe, "__sd_http_pipe"),
            (BridgeName::File, "__sd_file"),
            (BridgeName::Upload, "__sd_upload"),
            (BridgeName::State, "__sd_state"),
            (BridgeName::ValidateHeaders, "__sd_validate_headers"),
            (BridgeName::Crypto, "__sd_crypto"),
        ] {
            assert_eq!(name.as_str(), label);
            assert_eq!(serde_json::to_value(name).expect("serialize bridge"), label);
        }
    }

    #[test]
    fn protocol_line_reads_are_bounded() {
        let mut oversized = std::io::Cursor::new(b"123456789".to_vec());
        let error = read_limited_line_with_limit(&mut oversized, 8)
            .expect_err("oversized protocol line was accepted");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let mut allowed = std::io::Cursor::new(b"12345678".to_vec());
        assert_eq!(
            read_limited_line_with_limit(&mut allowed, 8).expect("read bounded line"),
            Some("12345678".to_string())
        );
    }

    #[tokio::test]
    async fn async_protocol_line_reads_are_bounded() {
        let oversized = b"123456789";
        let mut oversized = tokio::io::BufReader::new(&oversized[..]);
        let error = read_limited_line_async_with_limit(&mut oversized, 8)
            .await
            .expect_err("oversized protocol line was accepted");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let allowed = b"12345678";
        let mut allowed = tokio::io::BufReader::new(&allowed[..]);
        assert_eq!(
            read_limited_line_async_with_limit(&mut allowed, 8)
                .await
                .expect("read bounded line"),
            Some("12345678".to_string())
        );
    }

    #[test]
    fn unknown_upstream_error_kinds_are_not_coerced() {
        assert_eq!(
            FinalErrorKind::from_upstream("timeout"),
            Some(FinalErrorKind::Timeout)
        );
        assert_eq!(
            FinalErrorKind::from_upstream("dns"),
            Some(FinalErrorKind::Dns)
        );
        assert_eq!(
            FinalErrorKind::from_upstream("transport"),
            Some(FinalErrorKind::Transport)
        );
        assert_eq!(FinalErrorKind::from_upstream("quantum"), None);
    }

    #[test]
    fn poisoned_state_store_returns_a_catchable_internal_error() {
        let shared_state = state::new_shared_state();
        let poisoned = Arc::clone(&shared_state);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("poison state store");
            panic!("poison state store");
        })
        .join();

        let result = dispatch_state(&shared_state, &json!({ "op": "get", "key": "key" }));
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("state_internal_error"));
        assert_eq!(result["message"], json!("state_internal_error"));
    }

    #[test]
    fn concurrent_state_take_and_set_if_absent_have_exactly_one_winner() {
        let shared_state = state::new_shared_state();
        let seeded = dispatch_state(
            &shared_state,
            &json!({ "op": "set", "key": "take:key", "value": "grant" }),
        );
        assert_eq!(seeded["ok"], json!(true));

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let attempts = (0..2)
            .map(|_| {
                let shared_state = Arc::clone(&shared_state);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    dispatch_state(&shared_state, &json!({ "op": "take", "key": "take:key" }))
                })
            })
            .collect::<Vec<_>>();
        let results = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("take thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| result["found"] == json!(true))
                .count(),
            1
        );

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let attempts = ["left", "right"]
            .into_iter()
            .map(|value| {
                let shared_state = Arc::clone(&shared_state);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    dispatch_state(
                        &shared_state,
                        &json!({
                            "op": "setIfAbsent",
                            "key": "request:key",
                            "value": value
                        }),
                    )
                })
            })
            .collect::<Vec<_>>();
        let results = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("setIfAbsent thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| result["stored"] == json!(true))
                .count(),
            1
        );
    }

    #[test]
    fn final_result_round_trips_success_and_error_shapes() {
        let success = WorkerMessage::FinalResult(FinalResult::Success(json!({ "response": null })));
        assert_round_trip(
            &success,
            &json!({
                "type": "final_result",
                "ok": true,
                "result": { "response": null }
            }),
        );

        let failure = WorkerMessage::FinalResult(FinalResult::Error(FinalError {
            kind: Some(FinalErrorKind::Dns),
            message: "lookup failed".into(),
        }));
        assert_round_trip(
            &failure,
            &json!({
                "type": "final_result",
                "ok": false,
                "error": { "kind": "dns", "message": "lookup failed" }
            }),
        );
    }

    #[test]
    fn unknown_protocol_variants_are_rejected() {
        for message in [
            json!({ "type": "alien" }),
            json!({ "type": "host_call", "name": "__sd_unknown", "payload": {} }),
            json!({
                "type": "final_result",
                "ok": false,
                "error": { "kind": "quantum", "message": "boom" }
            }),
        ] {
            assert!(
                serde_json::from_value::<WorkerMessage>(message).is_err(),
                "unknown variant was accepted"
            );
        }
        assert!(serde_json::from_value::<ParentMessage>(json!({ "type": "alien" })).is_err());
    }

    #[test]
    fn malformed_final_result_shapes_are_rejected() {
        for message in [
            json!({ "type": "final_result", "ok": true }),
            json!({ "type": "final_result", "ok": false }),
            json!({
                "type": "final_result",
                "ok": true,
                "result": null,
                "error": { "message": "boom" }
            }),
            json!({
                "type": "final_result",
                "ok": false,
                "result": null,
                "error": { "message": "boom" }
            }),
        ] {
            assert!(
                serde_json::from_value::<WorkerMessage>(message.clone()).is_err(),
                "malformed final_result was accepted: {message}"
            );
        }
    }

    fn assert_round_trip<T>(value: &T, wire: &Json)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let encoded = serde_json::to_value(value).expect("serialize protocol message");
        assert_eq!(&encoded, wire);
        let decoded = serde_json::from_value::<T>(encoded).expect("deserialize protocol message");
        assert_eq!(&decoded, value);
    }

    #[test]
    fn engine_panic_is_isolated_from_the_caller() {
        // No script can deterministically panic Boa 0.22 through the public
        // API, so containment is locked at the guard itself; the black-box
        // suite covers engine errors and cross-route stability.
        let outcome = guard_engine(|| -> Result<(), ()> { panic!("engine bug") });
        let Err(error) = outcome else {
            panic!("panic escaped the guard");
        };
        let Error::Failed(message) = &error else {
            panic!("unexpected guard error: {error:?}");
        };
        assert_eq!(message, "script panicked in the engine");
        assert_eq!(error.class(), "script_error");
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    #[allow(unknown_lints, clippy::assert_is_empty)]
    fn parse_query_decodes_keys_and_values() {
        let query = parse_query("format=csv&%7Bgroup%7D=a+b&flag");
        assert_eq!(query.get("format").map(String::as_str), Some("csv"));
        assert_eq!(query.get("{group}").map(String::as_str), Some("a b"));
        assert_eq!(query.get("flag").map(String::as_str), Some(""));
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn request_snapshot_normalizes_method_path_and_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Request-ID", "caller".parse().expect("header value"));
        let snapshot = RequestSnapshot::new(
            &Method::POST,
            "/demo/1",
            HashMap::from([("group".to_string(), "a".to_string())]),
            "x=1",
            &headers,
            Some(b"body"),
            Vec::new(),
        );
        assert_eq!(snapshot.method, "POST");
        assert_eq!(snapshot.params["group"], "a");
        assert_eq!(snapshot.headers[0].0, "x-request-id");
        assert_eq!(snapshot.body_text.as_deref(), Some("body"));
    }

    #[test]
    fn non_utf8_body_is_null() {
        let snapshot = RequestSnapshot::new(
            &Method::POST,
            "/demo/1",
            HashMap::new(),
            "",
            &HeaderMap::new(),
            Some(&[0xFF, 0xFE]),
            Vec::new(),
        );
        assert!(snapshot.body_text.is_none());
    }

    #[test]
    fn error_classes_are_stable() {
        assert_eq!(Error::NoResponse.class(), "script_no_response");
        assert_eq!(Error::TimedOut.class(), "script_error");
        assert_eq!(Error::MemoryLimitExceeded.class(), "script_error");
        assert_eq!(
            Error::MemoryLimitExceeded.detail(),
            "script exceeded the configured memory limit"
        );
        assert_eq!(Error::Failed("boom".into()).class(), "script_error");
        assert_eq!(Error::CapacityExceeded.class(), "script_error");
        assert_eq!(
            Error::CapacityExceeded.detail(),
            "script worker capacity exhausted"
        );
    }

    #[test]
    fn engine_oom_errors_are_classified_as_memory_limit_errors() {
        let allocation_failure: JsError = JsNativeError::range()
            .with_message(
                "invalid layout Layout { size: 16777216, align: 1 } while allocating data block",
            )
            .into();
        assert!(engine_out_of_memory(&allocation_failure));

        let capacity_overflow: JsError = JsNativeError::range()
            .with_message(
                "capacity overflow for size 18446744073709551615 while allocating data block",
            )
            .into();
        assert!(!engine_out_of_memory(&capacity_overflow));

        let datum_conversion: JsError = JsNativeError::range()
            .with_message("couldn't allocate the data block: out of range")
            .into();
        assert!(!engine_out_of_memory(&datum_conversion));

        let unrelated: JsError = JsNativeError::range()
            .with_message("unrelated range error")
            .into();
        assert!(!engine_out_of_memory(&unrelated));
    }

    #[test]
    fn allocator_abort_lines_are_recognized_exactly() {
        assert!(is_allocator_abort_line(
            "memory allocation of 16777216 bytes failed"
        ));
        assert!(!is_allocator_abort_line(
            "prefix memory allocation of 16777216 bytes failed"
        ));
        assert!(!is_allocator_abort_line(
            "memory allocation of 16777216 bytes failed suffix"
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn allocation_abort_is_attributed_to_the_memory_limit() {
        let diagnostics = WorkerDiagnostics {
            memory_allocation_failed: true,
        };
        assert!(matches!(
            unexpected_worker_error(&diagnostics),
            Error::MemoryLimitExceeded
        ));
        assert!(matches!(
            unexpected_worker_error(&WorkerDiagnostics::default()),
            Error::WorkerTerminated
        ));
    }
}
