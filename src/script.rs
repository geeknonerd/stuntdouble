// Script execution: run one route script in Boa with a host-injected `ctx`.
// Contracts: docs/contracts/ctx-api.md (subset implemented by slice T2).
//
// The crate denies `unsafe`, and Boa 0.22 only exposes native closures through
// `unsafe fn NativeFunction::from_closure`. So instead of registering Rust
// callbacks, the host evaluates a small prelude that builds `ctx` in the realm
// on top of a JSON snapshot, then reads the produced response back with
// `JSON.stringify`. Scripts still see nothing but `ctx`: the realm has no
// `fetch`, `fs`, `process`, or `require`.
use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use axum::http::{HeaderMap, Method};
use boa_engine::{Context, Source};
use serde_json::{json, Value as Json};

use crate::matcher::percent_decode;

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
}

impl Error {
    /// Stable error class used in responses and logs.
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            Self::NoResponse => "script_no_response",
            Self::Failed(_) | Self::TimedOut => "script_error",
        }
    }

    /// Operator-facing reason; only exposed to clients under `--verbose` (T7).
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Failed(message) => message.clone(),
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
var ctx = (function () {
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
  return Object.freeze({
    apiVersion: "__SD_API_VERSION__",
    request: freezeShallow(__SD_REQUEST__),
    env: Object.freeze(__SD_ENV__),
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
})();
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
fn evaluate(source: &str, request: &RequestSnapshot) -> Outcome {
    let prelude = PRELUDE
        .replace("__SD_API_VERSION__", API_VERSION)
        .replace("__SD_REQUEST__", &js_literal(&request.to_json()))
        .replace("__SD_ENV__", &js_literal(&env_json()));
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    // A Rust-side panic in the engine must not take the request thread down.
    let staged = catch_unwind(AssertUnwindSafe(|| {
        context
            .eval(Source::from_bytes(&prelude))
            .map_err(|error| error.to_string())?;
        context
            .eval(Source::from_bytes(source))
            .map_err(|error| error.to_string())?;
        let dump = context
            .eval(Source::from_bytes(EXTRACT))
            .map_err(|error| error.to_string())?;
        Ok::<String, String>(match dump.as_string() {
            Some(text) => text.to_std_string_escaped(),
            None => "null".to_string(),
        })
    }));
    match staged {
        Err(_) => Outcome::failed(Error::Failed("script panicked in the engine".into())),
        Ok(Err(message)) => Outcome::failed(Error::Failed(message)),
        Ok(Ok(dump)) => parse_host_record(&dump),
    }
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
pub async fn execute(source: String, request: RequestSnapshot, timeout: Duration) -> Outcome {
    // tradeoff: `spawn_blocking` cannot be cancelled, so on timeout the host
    // answers immediately and the worker stops at `LOOP_ITERATION_LIMIT`.
    let worker = tokio::task::spawn_blocking(move || evaluate(&source, &request));
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
