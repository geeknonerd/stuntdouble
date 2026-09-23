// Script execution: run one route script in Boa with a host-injected `ctx`.
// Contracts: docs/contracts/ctx-api.md (subset implemented by slices T2–T4).
//
// The crate denies `unsafe`. Boa 0.22 only exposes native closures through
// `unsafe fn NativeFunction::from_closure`, so the only Rust callback is a safe
// `NativeFunction::from_fn_ptr` bridge with per-request state in a thread-local.
// The prelude builds `ctx` in the realm from JSON snapshots and reads the
// produced response back with `JSON.stringify`. Scripts still see nothing but
// `ctx`: the realm has no `fetch`, `fs`, `process`, or `require`, and the raw
// bridge global is deleted before the route script runs.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use boa_engine::native_function::NativeFunction;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsString, JsValue, Source};
use serde_json::{json, Value as Json};

use crate::config::{FilesConfig, UpstreamConfig};
use crate::files;
use crate::matcher::percent_decode;
use crate::upstream;

/// Value of `ctx.apiVersion`; see docs/contracts/ctx-api.md.
pub const API_VERSION: &str = "1";

// One script run gets a fixed resource envelope. Boa 0.22 exposes no interrupt
// hook and no heap metric or limit (see the T3 amendment in
// plans/adr/0003-script-first-multi-runtime.md), so the enforceable bounds are
// the wall-clock deadline that answers the client, the loop-iteration backstop
// that eventually stops the orphaned worker, and the VM recursion/stack limits
// that keep runaway recursion from exhausting the host stack.
// tradeoff: a worker abandoned at the deadline keeps running until the
// iteration backstop trips; a true heap cap needs a future Boa observation
// point or process isolation. Upgrade path: use the engine's interrupter and
// heap metrics once the pinned release exposes them.
const LOOP_ITERATION_LIMIT: u64 = 100_000_000;
const RECURSION_LIMIT: usize = 512;
const VM_STACK_SIZE_LIMIT: usize = 10_240;

/// Failure payload shared by the host bridge and its panic-catching wrapper:
/// an optional transport kind plus the operator-facing message.
type BridgeError = (Option<&'static str>, String);

/// Read-only view of one client request, frozen into `ctx.request`.
#[derive(Debug, Clone)]
pub struct RequestSnapshot {
    pub method: String,
    pub path: String,
    pub params: HashMap<String, String>,
    pub query: BTreeMap<String, String>,
    pub headers: Vec<(String, String)>,
    pub body_text: Option<String>,
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
        body: &[u8],
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
            body_text: std::str::from_utf8(body).map(str::to_string).ok(),
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
    /// Script threw, the engine aborted, or the worker vanished.
    Failed(String),
    /// Wall-clock deadline exceeded.
    TimedOut,
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
            Self::Failed(_) | Self::TimedOut => "script_error",
            Self::UpstreamUnreachable { .. } => "upstream_unreachable",
        }
    }

    /// HTTP status used when this error reaches the client uncaught.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::UpstreamUnreachable { .. } => StatusCode::BAD_GATEWAY,
            Self::Failed(_) | Self::TimedOut | Self::NoResponse => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Client-visible diagnostic under `--verbose`: a stable class only, never
    /// a stack trace, script message, upstream body, or upstream address.
    #[must_use]
    pub fn detail(&self) -> &'static str {
        match self {
            Self::Failed(_) => "script execution failed",
            Self::TimedOut => "script exceeded the configured timeout",
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
var ctx = (function (__sd_http_get, __sd_http_pipe, __sd_validate_headers, __sd_upstream_marker, __sd_file) {
  "use strict";
  var state = __sd;
  var FILE_HANDLES = new WeakMap();
  function format(value) {
    if (typeof value === "string") { return value; }
    if (value === undefined) { return "undefined"; }
    if (value === null) { return "null"; }
    try { return JSON.stringify(value); } catch (e) { return String(value); }
  }
  function record(level, args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) { parts.push(format(args[i])); }
    state.logs.push({ level: level, message: parts.join(" ") });
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
      var handle = FILE_HANDLES.get(body);
      if (handle !== undefined) { return { file: handle }; }
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
      if (value < 0) { throw new TypeError("ctx.http.get: invalid base64 body"); }
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
  function checkPlainOpts(opts, label, allowedKeys) {
    if (opts === undefined) { return {}; }
    if (opts === null || typeof opts !== "object" || Array.isArray(opts)) {
      throw new TypeError(label + ": opts must be an object");
    }
    var proto = Object.getPrototypeOf(opts);
    if (proto !== Object.prototype && proto !== null) {
      throw new TypeError(label + ": opts must be a plain object");
    }
    var symbols = Object.getOwnPropertySymbols(opts);
    if (symbols.length > 0) {
      throw new TypeError(label + ": unknown opts key " + String(symbols[0]));
    }
    var keys = Object.getOwnPropertyNames(opts);
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
    FILE_HANDLES.set(handle, result.handle);
    return handle;
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
    if (state.response !== null) {
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
    state.response = {
      stream: true,
      status: result.status,
      headers: result.headers
    };
    return true;
  }
  return Object.freeze({
    apiVersion: "__SD_API_VERSION__",
    request: freezeShallow(__SD_REQUEST__),
    env: Object.freeze(__SD_ENV__),
    http: Object.freeze({ get: httpGet, pipe: httpPipe }),
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
      if (state.response !== null) {
        record("warn", ["ctx.respond ignored: a response was already produced"]);
        return false;
      }
      var checkedStatus = checkStatus(status, "ctx.respond");
      var checkedHeaders = checkHeaders(headers, "ctx.respond");
      var checkedBody = checkBody(body);
      if (typeof checkedBody === "object" && checkedBody !== null && checkedBody.file !== undefined) {
        checkFileStreamResponse(checkedStatus, checkedHeaders);
        FILE_HANDLES.delete(body);
      }
      state.response = {
        status: checkedStatus,
        headers: checkedHeaders,
        body: checkedBody
      };
      return true;
    }
  });
})(__sd_http_get, __sd_http_pipe, __sd_validate_headers, __SD_UPSTREAM_MARKER__, __sd_file);
delete globalThis.__sd_http_get;
delete globalThis.__sd_http_pipe;
delete globalThis.__sd_validate_headers;
delete globalThis.__sd_file;
"#;

/// Read the recorded response and log lines back out of the realm.
const EXTRACT: &str = "JSON.stringify({ response: __sd.response, logs: __sd.logs })";

/// Serialize a value as a JS literal, keeping the injected source line-safe.
fn js_literal(value: &Json) -> String {
    value
        .to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

thread_local! {
    /// Per-request bridge used by the `__sd_http_get` and `__sd_http_pipe`
    /// native callbacks.
    static HTTP_HOST: RefCell<Option<upstream::UpstreamAccess>> = const { RefCell::new(None) };

    /// Per-request piped body captured by `__sd_http_pipe` and claimed by
    /// `parse_host_record` once the script finishes.
    static PIPE_STREAM: RefCell<Option<upstream::PipeBody>> = const { RefCell::new(None) };

    /// Per-request file access installed before the route script runs.
    static FILE_HOST: RefCell<Option<files::FileAccess>> = const { RefCell::new(None) };
}

/// Decode one host-bridge argument and encode the JSON result for the script.
/// Every native callback shares this wrapper so the fail-closed argument and
/// error shapes cannot drift.
fn host_bridge(
    name: &str,
    args: &[JsValue],
    call: impl FnOnce(&Json) -> Json,
) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message(format!("{name}: expected a JSON string"))
            .into());
    };
    let payload: Json = serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
        JsNativeError::typ().with_message(format!("{name}: invalid payload: {error}"))
    })?;
    let result = call(&payload);
    let rendered = serde_json::to_string(&result).map_err(|error| {
        JsNativeError::error().with_message(format!("{name}: cannot encode result: {error}"))
    })?;
    Ok(JsValue::from(JsString::from(rendered)))
}

/// Run one host closure against a request-scoped thread-local, answering a
/// fail-closed script error when a bridge is called outside a request.
fn host_call<T>(
    name: &str,
    cell: &'static std::thread::LocalKey<RefCell<Option<T>>>,
    payload: &Json,
    run: impl FnOnce(&T, &Json) -> Json,
) -> Json {
    cell.with(|slot| {
        let borrowed = slot.borrow();
        borrowed.as_ref().map_or_else(
            || {
                json!({
                    "ok": false,
                    "code": "script_error",
                    "message": format!("{name} called outside a script request")
                })
            },
            |host| run(host, payload),
        )
    })
}

/// Native bridge behind `ctx.http.get`; all policy checks live in `upstream`.
fn sd_http_get(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    host_bridge("__sd_http_get", args, |call| {
        host_call("__sd_http_get", &HTTP_HOST, call, |host, call| {
            match host.get(call) {
                Ok(response) => {
                    json!({ "ok": true, "response": upstream::response_json(response) })
                }
                Err(error) => json!({
                    "ok": false,
                    "code": error.code(),
                    "kind": error.transport_kind(),
                    "message": error.message()
                }),
            }
        })
    })
}

/// Native bridge behind `ctx.http.pipe`; all policy checks live in `upstream`.
/// The streamed body stays in this thread-local until the script ends, so the
/// JavaScript side only ever records the client status and headers.
fn sd_http_pipe(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    host_bridge("__sd_http_pipe", args, |call| {
        host_call(
            "__sd_http_pipe",
            &HTTP_HOST,
            call,
            |host, call| match host.pipe(call) {
                Ok(response) => {
                    PIPE_STREAM.with(|cell| {
                        *cell.borrow_mut() = Some(response.body);
                    });
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
                Err(error) => json!({
                    "ok": false,
                    "code": error.code(),
                    "kind": error.transport_kind(),
                    "message": error.message()
                }),
            },
        )
    })
}

/// Native bridge behind `ctx.file.*`; all confinement checks live in `files`.
fn sd_file(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    host_bridge("__sd_file", args, |call| {
        host_call("__sd_file", &FILE_HOST, call, files::FileAccess::call)
    })
}

/// Native bridge used by the prelude to validate script-supplied response
/// headers before `ctx.respond` stores them or `ctx.http.pipe` starts an
/// upstream call. The script sees a catchable `script_error`.
fn sd_validate_headers(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message("__sd_validate_headers: expected a JSON string")
            .into());
    };
    let pairs: Vec<(String, String)> =
        serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
            JsNativeError::typ()
                .with_message(format!("__sd_validate_headers: invalid payload: {error}"))
        })?;
    let result = match validated_header_pairs(&pairs) {
        Ok(_) => json!({ "ok": true }),
        Err(message) => json!({
            "ok": false,
            "code": "script_error",
            "message": message,
        }),
    };
    Ok(JsValue::from(JsString::from(result.to_string())))
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

/// Evaluate one request's script and read back its recorded state.
fn evaluate(
    source: &str,
    request: &RequestSnapshot,
    upstream: &UpstreamConfig,
    files_config: &FilesConfig,
    script_deadline: Instant,
    calls: upstream::CallLog,
    file_calls: files::CallLog,
) -> Outcome {
    let client_range = request
        .headers
        .iter()
        .find(|(name, _)| name == "range")
        .map(|(_, value)| value.clone());
    let file_access = files::FileAccess::new(&files_config.root, file_calls);
    HTTP_HOST.with(|cell| {
        *cell.borrow_mut() = Some(upstream::UpstreamAccess::new(
            upstream.allow_hosts.clone(),
            Duration::from_millis(upstream.timeout_ms),
            script_deadline,
            client_range,
            calls,
        ));
    });
    FILE_HOST.with(|cell| {
        *cell.borrow_mut() = Some(file_access);
    });
    PIPE_STREAM.with(|cell| {
        *cell.borrow_mut() = None;
    });
    let upstream_marker = upstream_marker();
    let prelude = PRELUDE
        .replace("__SD_API_VERSION__", API_VERSION)
        .replace("__SD_REQUEST__", &js_literal(&request.to_json()))
        .replace("__SD_ENV__", &js_literal(&env_json()))
        .replace(
            "__SD_UPSTREAM_MARKER__",
            &js_literal(&Json::String(upstream_marker.clone())),
        );
    let mut context = Context::default();
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);
    limits.set_stack_size_limit(VM_STACK_SIZE_LIMIT);
    // A Rust-side panic in the engine must not take the request thread down.
    let staged = guard_engine(|| -> Result<String, BridgeError> {
        context
            .register_global_builtin_callable(
                js_string!("__sd_http_get"),
                1,
                NativeFunction::from_fn_ptr(sd_http_get),
            )
            .map_err(|error| (None, error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_http_pipe"),
                1,
                NativeFunction::from_fn_ptr(sd_http_pipe),
            )
            .map_err(|error| (None, error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_validate_headers"),
                1,
                NativeFunction::from_fn_ptr(sd_validate_headers),
            )
            .map_err(|error| (None, error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_file"),
                1,
                NativeFunction::from_fn_ptr(sd_file),
            )
            .map_err(|error| (None, error.to_string()))?;
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
            Err(error) => {
                let kind = upstream_unreachable_kind(&error, &upstream_marker, &mut context);
                let message = error.to_string();
                Err((kind, message))
            }
        }
    });
    let stream = PIPE_STREAM.with(|cell| cell.borrow_mut().take());
    let file_host = FILE_HOST.with(|cell| cell.borrow_mut().take());
    HTTP_HOST.with(|cell| {
        cell.borrow_mut().take();
    });
    match staged {
        Err(error) => Outcome::failed(error),
        Ok(Err((Some(kind), message))) => {
            Outcome::failed(Error::UpstreamUnreachable { message, kind })
        }
        Ok(Err((None, message))) => Outcome::failed(Error::Failed(message)),
        Ok(Ok(dump)) => parse_host_record(&dump, stream, file_host),
    }
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
    raw: &str,
    pipe_stream: Option<upstream::PipeBody>,
    file_host: Option<files::FileAccess>,
) -> Outcome {
    let host: Json = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            return Outcome::failed(Error::Failed(format!("cannot read script state: {error}")));
        }
    };
    let logs = parse_script_logs(&host);
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
    let body = match parse_script_body(response, pipe_stream, file_host) {
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

/// Run one script for one request with a wall-clock deadline.
///
/// Must use result: an unreachable engine is reported, never a silent success.
pub async fn execute(
    source: String,
    request: RequestSnapshot,
    timeout: Duration,
    upstream: UpstreamConfig,
    files_config: FilesConfig,
) -> Outcome {
    // tradeoff: `spawn_blocking` cannot be cancelled, so on timeout the host
    // answers immediately and the worker stops at `LOOP_ITERATION_LIMIT`.
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let calls: upstream::CallLog = Arc::new(Mutex::new(Vec::new()));
    let worker_calls = Arc::clone(&calls);
    let file_calls: files::CallLog = Arc::new(Mutex::new(Vec::new()));
    let worker_file_calls = Arc::clone(&file_calls);
    let worker = tokio::task::spawn_blocking(move || {
        evaluate(
            &source,
            &request,
            &upstream,
            &files_config,
            deadline,
            worker_calls,
            worker_file_calls,
        )
    });
    let mut outcome = match tokio::time::timeout(timeout, worker).await {
        Err(_) => Outcome::failed(Error::TimedOut),
        Ok(Err(_)) => Outcome::failed(Error::Failed("script worker panicked".into())),
        Ok(Ok(outcome)) => outcome,
    };
    outcome.upstream_calls = upstream::calls_json(&calls);
    outcome.file_calls = file_calls;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

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
            b"body",
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
            &[0xFF, 0xFE],
        );
        assert!(snapshot.body_text.is_none());
    }

    #[test]
    fn error_classes_are_stable() {
        assert_eq!(Error::NoResponse.class(), "script_no_response");
        assert_eq!(Error::TimedOut.class(), "script_error");
        assert_eq!(Error::Failed("boom".into()).class(), "script_error");
    }
}
