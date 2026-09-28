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
use std::io::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use boa_engine::native_function::NativeFunction;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsNativeErrorKind, JsString, JsValue, Source,
};
use serde_json::{json, Value as Json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::{FilesConfig, SandboxConfig, UpstreamConfig, SCRIPT_MEMORY_LIMIT_FLOOR_MB};
use crate::files;
use crate::matcher::percent_decode;
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
    /// The worker exceeded the configured Linux virtual-address-space bound.
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
var ctx = (function (__sd_http_get, __sd_http_pipe, __sd_validate_headers, __sd_upstream_marker, __sd_file, __sd_upload) {
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
    request: requestSnapshot(__SD_REQUEST__),
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
      if (typeof checkedBody === "object" && checkedBody !== null &&
          (checkedBody.file !== undefined || checkedBody.upload !== undefined)) {
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
})(__sd_http_get, __sd_http_pipe, __sd_validate_headers, __SD_UPSTREAM_MARKER__, __sd_file, __sd_upload);
delete globalThis.__sd_http_get;
delete globalThis.__sd_http_pipe;
delete globalThis.__sd_validate_headers;
delete globalThis.__sd_file;
delete globalThis.__sd_upload;
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

/// Write one JSON Lines protocol message to the parent.
fn write_protocol_line(value: &Json) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, value).map_err(std::io::Error::other)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

/// Read one protocol line from the parent. `None` is EOF before any line.
fn read_protocol_line() -> std::io::Result<Option<String>> {
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line)?;
    Ok((read > 0).then_some(line))
}

/// Send one host call and wait for the parent's exactly-one result.
fn worker_host_call(name: &str, payload: &Json) -> boa_engine::JsResult<Json> {
    let call = json!({ "type": "host_call", "name": name, "payload": payload });
    write_protocol_line(&call).map_err(|error| {
        JsNativeError::error().with_message(format!("{name}: cannot reach host: {error}"))
    })?;
    let line = read_protocol_line().map_err(|error| {
        JsNativeError::error().with_message(format!("{name}: host read failed: {error}"))
    })?;
    let line = line.ok_or_else(|| {
        JsNativeError::error().with_message(format!("{name}: host closed the pipe"))
    })?;
    let mut message: Json = serde_json::from_str(&line).map_err(|error| {
        JsNativeError::error().with_message(format!("{name}: invalid host response: {error}"))
    })?;
    drop(line);
    if message.get("type").and_then(Json::as_str) != Some("host_result") {
        return Err(JsNativeError::error()
            .with_message(format!("{name}: expected host_result"))
            .into());
    }
    message
        .get_mut("result")
        .map(std::mem::take)
        .ok_or_else(|| {
            JsNativeError::error()
                .with_message(format!("{name}: host_result is missing result"))
                .into()
        })
}

/// Decode one bridge argument, dispatch it to the parent, and encode the JSON
/// string the script prelude expects.
fn worker_bridge(name: &str, args: &[JsValue]) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message(format!("{name}: expected a JSON string"))
            .into());
    };
    let payload: Json = serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
        JsNativeError::typ().with_message(format!("{name}: invalid payload: {error}"))
    })?;
    let result = worker_host_call(name, &payload)?;
    let rendered = serde_json::to_string(&result).map_err(|error| {
        JsNativeError::error().with_message(format!("{name}: cannot encode result: {error}"))
    })?;
    drop(result);
    Ok(JsValue::from(JsString::from(rendered)))
}

fn worker_http_get(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge("__sd_http_get", args)
}

fn worker_http_pipe(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge("__sd_http_pipe", args)
}

fn worker_file(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge("__sd_file", args)
}

fn worker_upload(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge("__sd_upload", args)
}

fn worker_validate_headers(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    worker_bridge("__sd_validate_headers", args)
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

/// Serialize one request and environment snapshot into the script prelude.
fn prelude_for_request(request: &Json, upstream_marker: &str) -> String {
    PRELUDE
        .replace("__SD_API_VERSION__", API_VERSION)
        .replace("__SD_REQUEST__", &js_literal(request))
        .replace("__SD_ENV__", &js_literal(&env_json()))
        .replace(
            "__SD_UPSTREAM_MARKER__",
            &js_literal(&Json::String(upstream_marker.to_string())),
        )
}

/// Evaluate one script inside the worker process. Every host function is
/// forwarded to the parent; upstream, file, and upload policy stay there.
fn evaluate_in_worker(source: &str, request: &Json) -> Result<Json, Error> {
    let upstream_marker = upstream_marker();
    let prelude = prelude_for_request(request, &upstream_marker);
    let mut context = Context::default();
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);
    limits.set_stack_size_limit(VM_STACK_SIZE_LIMIT);
    // A Rust-side panic in the engine must not take the worker down without a
    // protocol result; it is mapped to the same script failure as before.
    let staged = guard_engine(|| -> Result<String, Error> {
        context
            .register_global_builtin_callable(
                js_string!("__sd_http_get"),
                1,
                NativeFunction::from_fn_ptr(worker_http_get),
            )
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_http_pipe"),
                1,
                NativeFunction::from_fn_ptr(worker_http_pipe),
            )
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_validate_headers"),
                1,
                NativeFunction::from_fn_ptr(worker_validate_headers),
            )
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_file"),
                1,
                NativeFunction::from_fn_ptr(worker_file),
            )
            .map_err(|error| Error::Failed(error.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__sd_upload"),
                1,
                NativeFunction::from_fn_ptr(worker_upload),
            )
            .map_err(|error| Error::Failed(error.to_string()))?;
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
                None if cfg!(target_os = "linux") && engine_out_of_memory(&error) => {
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
}

impl HostState {
    fn new(
        upstream_config: &UpstreamConfig,
        files_config: &FilesConfig,
        uploads: Option<Arc<files::UploadStore>>,
        script_deadline: Instant,
        client_range: Option<String>,
        calls: upstream::CallLog,
        file_calls: files::CallLog,
    ) -> Self {
        let upstream = upstream::UpstreamAccess::new(
            upstream_config.allow_hosts.clone(),
            Duration::from_millis(upstream_config.timeout_ms),
            script_deadline,
            client_range,
            calls,
        );
        let files = files::FileAccess::new(&files_config.root, Arc::clone(&file_calls));
        let uploads = uploads.map(|store| files::UploadAccess::new(store, file_calls));
        Self {
            upstream: Some(upstream),
            files: Some(files),
            uploads,
            pipe: None,
        }
    }

    /// Handle one bridge call. `None` is an unknown bridge, which is a
    /// parent/worker protocol mismatch rather than a script error.
    fn dispatch(&mut self, name: &str, payload: &Json) -> Option<Json> {
        match name {
            "__sd_http_get" => {
                let host = self.upstream.as_ref()?;
                Some(match host.get(payload) {
                    Ok(response) => {
                        json!({ "ok": true, "response": upstream::response_json(response) })
                    }
                    Err(error) => upstream_error_json(&error),
                })
            }
            "__sd_http_pipe" => {
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
            "__sd_file" => Some(self.files.as_ref()?.call(payload)),
            "__sd_upload" => Some(self.uploads.as_ref().map_or_else(
                || {
                    json!({
                        "ok": false,
                        "code": "script_error",
                        "message": "__sd_upload called outside a script request"
                    })
                },
                |host| host.call(payload),
            )),
            "__sd_validate_headers" => Some(validate_headers_json(payload)),
            _ => None,
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
async fn dispatch_host(host: &Arc<Mutex<HostState>>, name: String, payload: Json) -> Option<Json> {
    let host = Arc::clone(host);
    tokio::task::spawn_blocking(move || {
        let mut host = host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        host.dispatch(&name, &payload)
    })
    .await
    .ok()
    .flatten()
}

/// Write one JSON Lines message to the worker.
async fn write_json_line(
    writer: &mut tokio::process::ChildStdin,
    value: &Json,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

enum WorkerReply {
    /// The worker delivered its one complete final result.
    Final(Json),
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
    job: &Json,
    deadline: tokio::time::Instant,
) -> WorkerReply {
    if write_json_line(&mut stdin, job).await.is_err() {
        return WorkerReply::Unexpected;
    }
    let mut lines = BufReader::new(stdout).lines();
    let mut final_result = None;
    loop {
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
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
        let Ok(message) = serde_json::from_str::<Json>(&line) else {
            return WorkerReply::Unexpected;
        };
        match message.get("type").and_then(Json::as_str) {
            Some("host_call") => {
                let Some(name) = message
                    .get("name")
                    .and_then(Json::as_str)
                    .map(str::to_string)
                else {
                    return WorkerReply::Unexpected;
                };
                let Some(payload) = message.get("payload").cloned() else {
                    return WorkerReply::Unexpected;
                };
                let result =
                    match tokio::time::timeout_at(deadline, dispatch_host(host, name, payload))
                        .await
                    {
                        Err(_) => return WorkerReply::TimedOut,
                        Ok(None) => return WorkerReply::Unexpected,
                        Ok(Some(result)) => result,
                    };
                let reply = json!({ "type": "host_result", "result": result });
                match tokio::time::timeout_at(deadline, write_json_line(&mut stdin, &reply)).await {
                    Err(_) => return WorkerReply::TimedOut,
                    Ok(Err(_)) => return WorkerReply::Unexpected,
                    Ok(Ok(())) => {}
                }
            }
            Some("final_result") => final_result = Some(message),
            _ => return WorkerReply::Unexpected,
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
    if cfg!(target_os = "linux") && diagnostics.memory_allocation_failed {
        Error::MemoryLimitExceeded
    } else {
        Error::WorkerTerminated
    }
}

/// Spawn one worker process and drive it until its final result or the shared
/// wall-clock deadline. The child is killed and reaped on every other exit.
async fn run_worker(
    job: Json,
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
        WorkerReply::Final(message) => {
            // The deadline still covers process teardown. A complete result is
            // consumed only after a successful exit; a lingering worker is
            // killed and reported as a timeout, and parent-held streams drop.
            match tokio::time::timeout_at(deadline, child.wait()).await {
                Ok(Ok(status)) if status.success() => {
                    let _ = stderr_task.await;
                    finish_outcome(&message, &host)
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

/// Map one complete `final_result` onto the existing Outcome shape.
fn finish_outcome(message: &Json, host: &Arc<Mutex<HostState>>) -> Outcome {
    let (pipe, file_host, upload_host) = {
        let mut host = host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        host.take_streams()
    };
    match message.get("ok").and_then(Json::as_bool) {
        Some(true) => match message.get("result") {
            Some(result) => parse_host_record(result, pipe, file_host, upload_host),
            None => Outcome::failed(Error::WorkerTerminated),
        },
        Some(false) => {
            let Some(error) = message.get("error") else {
                return Outcome::failed(Error::WorkerTerminated);
            };
            let Some(text) = error.get("message").and_then(Json::as_str) else {
                return Outcome::failed(Error::WorkerTerminated);
            };
            let error = match error.get("kind").and_then(Json::as_str) {
                Some("memory_limit") => Error::MemoryLimitExceeded,
                Some("timeout") => Error::UpstreamUnreachable {
                    message: text.to_string(),
                    kind: "timeout",
                },
                Some("dns") => Error::UpstreamUnreachable {
                    message: text.to_string(),
                    kind: "dns",
                },
                Some("transport") => Error::UpstreamUnreachable {
                    message: text.to_string(),
                    kind: "transport",
                },
                Some(_) => return Outcome::failed(Error::WorkerTerminated),
                None => Error::Failed(text.to_string()),
            };
            Outcome::failed(error)
        }
        None => Outcome::failed(Error::WorkerTerminated),
    }
}

/// Run one script for one request in a fresh worker process.
///
/// Must use result: a dead or unreachable worker is reported, never a silent
/// success. The semaphore permit is held for the whole worker lifetime.
pub async fn execute(
    source: String,
    request: RequestSnapshot,
    sandbox: SandboxConfig,
    upstream: UpstreamConfig,
    files_config: FilesConfig,
    uploads: Option<Arc<files::UploadStore>>,
    worker_slot: tokio::sync::OwnedSemaphorePermit,
) -> Outcome {
    let _slot = worker_slot;
    let timeout = Duration::from_millis(sandbox.script_timeout_ms);
    let script_deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let deadline = tokio::time::Instant::from_std(script_deadline);
    let calls: upstream::CallLog = Arc::new(Mutex::new(Vec::new()));
    let file_calls: files::CallLog = Arc::new(Mutex::new(Vec::new()));
    let client_range = request
        .headers
        .iter()
        .find(|(name, _)| name == "range")
        .map(|(_, value)| value.clone());
    let host = Arc::new(Mutex::new(HostState::new(
        &upstream,
        &files_config,
        uploads.clone(),
        script_deadline,
        client_range,
        Arc::clone(&calls),
        Arc::clone(&file_calls),
    )));
    let job = json!({
        "type": "job",
        "script": source,
        "request": request.to_json(),
    });
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
    outcome.upstream_calls = upstream::calls_json(&calls);
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

fn worker_main(memory_limit_mb: u64) -> std::io::Result<()> {
    if memory_limit_mb < SCRIPT_MEMORY_LIMIT_FLOOR_MB {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory limit is below the compiled-in floor",
        ));
    }
    // Apply the Linux bound before reading the job. Parsing a large script or
    // request snapshot is itself a meaningful allocation.
    // Other platforms deliberately keep process isolation and deadline kill
    // without claiming a hard memory bound.
    apply_worker_memory_limit(memory_limit_mb)?;
    let Some(line) = read_protocol_line()? else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "missing job line",
        ));
    };
    let job: Json = serde_json::from_str(&line)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if job.get("type").and_then(Json::as_str) != Some("job") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected a job line",
        ));
    }
    let Some(script) = job.get("script").and_then(Json::as_str) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "job script missing",
        ));
    };
    let Some(request) = job.get("request") else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "job request missing",
        ));
    };
    let message = match evaluate_in_worker(script, request) {
        Ok(result) => json!({ "type": "final_result", "ok": true, "result": result }),
        Err(Error::UpstreamUnreachable { message, kind }) => json!({
            "type": "final_result",
            "ok": false,
            "error": { "kind": kind, "message": message }
        }),
        Err(Error::Failed(message)) => json!({
            "type": "final_result",
            "ok": false,
            "error": { "message": message }
        }),
        Err(Error::MemoryLimitExceeded) => json!({
            "type": "final_result",
            "ok": false,
            "error": {
                "kind": "memory_limit",
                "message": Error::MemoryLimitExceeded.detail()
            }
        }),
        Err(error) => json!({
            "type": "final_result",
            "ok": false,
            "error": { "message": error.detail() }
        }),
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
    Ok(())
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

    #[cfg(target_os = "linux")]
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
