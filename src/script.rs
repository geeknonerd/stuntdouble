// Script execution: run one route script in Boa with a host-injected `ctx`.
// Contracts: docs/contracts/ctx-api.md (subset implemented by slices T2–T4).
//
// The crate denies `unsafe`, and Boa 0.22 only exposes native closures through
// `unsafe fn NativeFunction::from_closure`. So instead of registering Rust
// callbacks, the host evaluates a small prelude that builds `ctx` in the realm
// on top of a JSON snapshot, then reads the produced response back with
// `JSON.stringify`. Scripts still see nothing but `ctx`: the realm has no
// `fetch`, `fs`, `process`, or `require`.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, Method, StatusCode};
use boa_engine::native_function::NativeFunction;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsString, JsValue, Source};
use serde_json::{json, Value as Json};

use crate::config::UpstreamConfig;
use crate::matcher::percent_decode;
use crate::upstream;

/// Value of `ctx.apiVersion`; see docs/contracts/ctx-api.md.
pub const API_VERSION: &str = "1";

// tradeoff: Boa 0.22 exposes no interrupt hook, so a runaway script cannot be
// stopped at the wall-clock deadline; the client is answered at the deadline
// and the orphaned worker thread is bounded by this iteration backstop.
// Upgrade path: use the engine's interrupter once the pinned release has one.
const LOOP_ITERATION_LIMIT: u64 = 100_000_000;

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
#[derive(Debug, Clone)]
pub enum ResponseBody {
    Text(String),
    Bytes(Vec<u8>),
}

impl ResponseBody {
    /// Serialize the body for the HTTP layer.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Text(text) => text.into_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// Response produced by `ctx.respond`.
#[derive(Debug, Clone)]
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
    /// An uncaught transport failure from `ctx.http.get`.
    UpstreamUnreachable(String),
}

impl Error {
    /// Stable error class used in responses and logs.
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            Self::NoResponse => "script_no_response",
            Self::Failed(_) | Self::TimedOut => "script_error",
            Self::UpstreamUnreachable(_) => "upstream_unreachable",
        }
    }

    /// HTTP status used when this error reaches the client uncaught.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::UpstreamUnreachable(_) => StatusCode::BAD_GATEWAY,
            Self::Failed(_) | Self::TimedOut | Self::NoResponse => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Operator-facing reason; only exposed to clients under `--verbose` (T7).
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Failed(message) | Self::UpstreamUnreachable(message) => message.clone(),
            Self::TimedOut => "script exceeded sandbox.script_timeout_ms".into(),
            Self::NoResponse => "script finished without calling ctx.respond".into(),
        }
    }
}

/// Everything one script run produced.
#[derive(Debug)]
pub struct Outcome {
    pub response: Option<ScriptResponse>,
    pub logs: Vec<LogRecord>,
    pub error: Option<Error>,
}

impl Outcome {
    /// Report a failure that happened before the script could run.
    #[must_use]
    pub fn failed(error: Error) -> Self {
        Self {
            response: None,
            logs: Vec::new(),
            error: Some(error),
        }
    }
}

// The prelude is the only writer of `__sd`; `ctx` is a frozen view over it.
const PRELUDE: &str = r#"
var __sd = { response: null, logs: [] };
var ctx = (function (__sd_http_get, __sd_upstream_marker) {
  "use strict";
  var state = __sd;
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
  function checkStatus(status) {
    if (typeof status !== "number" || !Number.isInteger(status) || status < 100 || status > 599) {
      throw new TypeError("ctx.respond: status must be an integer in [100, 599]");
    }
    return status;
  }
  function checkHeaders(headers) {
    var out = [];
    if (headers === undefined || headers === null) { return out; }
    if (Array.isArray(headers)) {
      for (var i = 0; i < headers.length; i++) {
        var row = headers[i];
        if (!Array.isArray(row) || row.length !== 2) {
          throw new TypeError("ctx.respond: headers[" + i + "] must be a [name, value] pair");
        }
        out.push([String(row[0]), String(row[1])]);
      }
      return out;
    }
    if (typeof headers !== "object") {
      throw new TypeError("ctx.respond: headers must be an object or [name, value] pairs");
    }
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
    return out;
  }
  function checkBody(body) {
    if (body === undefined || body === null) { return ""; }
    if (typeof body === "string") { return body; }
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
  function makeUpstreamError(message) {
    var error = makeError(message, "upstream_unreachable");
    Object.defineProperty(error, __sd_upstream_marker, { value: true });
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
  function httpGet(url, opts) {
    if (typeof url !== "string") {
      throw new TypeError("ctx.http.get: url must be a string");
    }
    var timeout = null;
    if (opts !== undefined) {
      if (opts === null || typeof opts !== "object" || Array.isArray(opts)) {
        throw new TypeError("ctx.http.get: opts must be an object");
      }
      var proto = Object.getPrototypeOf(opts);
      if (proto !== Object.prototype && proto !== null) {
        throw new TypeError("ctx.http.get: opts must be a plain object");
      }
      var symbols = Object.getOwnPropertySymbols(opts);
      if (symbols.length > 0) {
        throw new TypeError("ctx.http.get: unknown opts key " + String(symbols[0]));
      }
      var keys = Object.getOwnPropertyNames(opts);
      for (var i = 0; i < keys.length; i++) {
        if (keys[i] !== "timeout_ms") {
          throw new TypeError("ctx.http.get: unknown opts key " + JSON.stringify(keys[i]));
        }
      }
      if (Object.prototype.hasOwnProperty.call(opts, "timeout_ms")) {
        var value = opts.timeout_ms;
        if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) {
          throw new TypeError("ctx.http.get: opts.timeout_ms must be a positive integer");
        }
        timeout = value;
      }
    }
    var payload = { url: url };
    if (timeout !== null) { payload.timeout_ms = timeout; }
    var raw;
    try {
      raw = __sd_http_get(JSON.stringify(payload));
    } catch (bridgeError) {
      throw makeError("ctx.http.get: host bridge failed", "script_error");
    }
    var result = JSON.parse(raw);
    if (!result.ok) {
      if (result.code === "upstream_unreachable") {
        throw makeUpstreamError(result.message);
      }
      throw makeError(result.message, result.code);
    }
    var upstream = result.response;
    return Object.freeze({
      status: upstream.status,
      headers: Object.freeze(upstream.headers),
      text: function () { return upstream.text; },
      bytes: function () { return decodeBase64(upstream.body_base64); }
    });
  }
  return Object.freeze({
    apiVersion: "__SD_API_VERSION__",
    request: freezeShallow(__SD_REQUEST__),
    env: Object.freeze(__SD_ENV__),
    http: Object.freeze({ get: httpGet }),
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
      state.response = {
        status: checkStatus(status),
        headers: checkHeaders(headers),
        body: checkBody(body)
      };
      return true;
    }
  });
})(__sd_http_get, __SD_UPSTREAM_MARKER__);
delete globalThis.__sd_http_get;
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
    /// Per-request bridge used by the `__sd_http_get` native callback.
    static HTTP_HOST: RefCell<Option<upstream::UpstreamAccess>> = const { RefCell::new(None) };
}

/// Native bridge behind `ctx.http.get`; all policy checks live in `upstream`.
fn sd_http_get(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(raw) = args.first().and_then(JsValue::as_string) else {
        return Err(JsNativeError::typ()
            .with_message("__sd_http_get: expected a JSON string")
            .into());
    };
    let call: Json = serde_json::from_str(&raw.to_std_string_escaped()).map_err(|error| {
        JsNativeError::typ().with_message(format!("__sd_http_get: invalid payload: {error}"))
    })?;
    let result = HTTP_HOST.with(|cell| {
        let borrowed = cell.borrow();
        let Some(host) = borrowed.as_ref() else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.http.get called outside a script request"
            });
        };
        match host.get(&call) {
            Ok(response) => json!({ "ok": true, "response": upstream::response_json(response) }),
            Err(error) => json!({
                "ok": false,
                "code": error.code(),
                "message": error.message()
            }),
        }
    });
    let rendered = serde_json::to_string(&result).map_err(|error| {
        JsNativeError::error().with_message(format!("__sd_http_get: cannot encode result: {error}"))
    })?;
    Ok(JsValue::from(JsString::from(rendered)))
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

/// Evaluate one request's script and read back its recorded state.
fn evaluate(
    source: &str,
    request: &RequestSnapshot,
    upstream: &UpstreamConfig,
    script_deadline: Instant,
) -> Outcome {
    HTTP_HOST.with(|cell| {
        *cell.borrow_mut() = Some(upstream::UpstreamAccess::new(
            upstream.allow_hosts.clone(),
            Duration::from_millis(upstream.timeout_ms),
            script_deadline,
        ));
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
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    // A Rust-side panic in the engine must not take the request thread down.
    let staged = catch_unwind(AssertUnwindSafe(|| -> Result<String, (bool, String)> {
        context
            .register_global_builtin_callable(
                js_string!("__sd_http_get"),
                1,
                NativeFunction::from_fn_ptr(sd_http_get),
            )
            .map_err(|error| (false, error.to_string()))?;
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
                let upstream_unreachable =
                    is_upstream_unreachable(&error, &upstream_marker, &mut context);
                let message = error.to_string();
                Err((upstream_unreachable, message))
            }
        }
    }));
    HTTP_HOST.with(|cell| {
        cell.borrow_mut().take();
    });
    match staged {
        Err(_) => Outcome::failed(Error::Failed("script panicked in the engine".into())),
        Ok(Err((true, message))) => Outcome::failed(Error::UpstreamUnreachable(message)),
        Ok(Err((false, message))) => Outcome::failed(Error::Failed(message)),
        Ok(Ok(dump)) => parse_host_record(&dump),
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

/// Recognize an uncaught error created by the `ctx.http.get` transport path.
/// The script-visible `error.code` is deliberately not trusted on its own.
fn is_upstream_unreachable(error: &JsError, marker: &str, context: &mut Context) -> bool {
    let Some(object) = error.as_opaque().and_then(JsValue::as_object) else {
        return false;
    };
    object
        .has_own_property(JsString::from(marker), context)
        .unwrap_or(false)
}

/// Turn the extracted JSON record into an `Outcome`.
fn parse_host_record(raw: &str) -> Outcome {
    let host: Json = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            return Outcome::failed(Error::Failed(format!("cannot read script state: {error}")));
        }
    };
    let logs = host
        .get("logs")
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
        });
    let Some(response) = host.get("response").filter(|value| !value.is_null()) else {
        return Outcome {
            response: None,
            logs,
            error: Some(Error::NoResponse),
        };
    };
    let Some(status) = response
        .get("status")
        .and_then(Json::as_u64)
        .and_then(|value| u16::try_from(value).ok())
    else {
        return Outcome {
            response: None,
            logs,
            error: Some(Error::Failed("ctx.respond: status was not recorded".into())),
        };
    };
    let headers = response
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
        });
    let body = match response.get("body") {
        Some(Json::String(text)) => ResponseBody::Text(text.clone()),
        Some(Json::Array(bytes)) => ResponseBody::Bytes(
            bytes
                .iter()
                .filter_map(Json::as_u64)
                .filter_map(|value| u8::try_from(value).ok())
                .collect(),
        ),
        _ => ResponseBody::Text(String::new()),
    };
    Outcome {
        response: Some(ScriptResponse {
            status,
            headers,
            body,
        }),
        logs,
        error: None,
    }
}

/// Run one script for one request with a wall-clock deadline.
///
/// Must use result: an unreachable engine is reported, never a silent success.
pub async fn execute(
    source: String,
    request: RequestSnapshot,
    timeout: Duration,
    upstream: UpstreamConfig,
) -> Outcome {
    // tradeoff: `spawn_blocking` cannot be cancelled, so on timeout the host
    // answers immediately and the worker stops at `LOOP_ITERATION_LIMIT`.
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let worker =
        tokio::task::spawn_blocking(move || evaluate(&source, &request, &upstream, deadline));
    match tokio::time::timeout(timeout, worker).await {
        Err(_) => Outcome::failed(Error::TimedOut),
        Ok(Err(_)) => Outcome::failed(Error::Failed("script worker panicked".into())),
        Ok(Ok(outcome)) => outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
