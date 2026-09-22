//! Allowlisted upstream HTTP calls for `ctx.http.get` and `ctx.http.pipe`.
//! Contract: docs/contracts/ctx-api.md
//!
//! The script host is synchronous, so this module uses ureq's blocking client.
//! Redirects are disabled at the client and followed here, one hop at a time,
//! so every target is re-validated against the allowlist before it is fetched.
//! `ctx.http.pipe` hands the upstream body to the HTTP layer as a bounded
//! channel of frames, so bytes never enter the script heap.
use std::cell::OnceCell;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value as Json};
use url::{Host as UrlHost, Url};

/// Maximum redirects followed after the initial request.
const MAX_REDIRECTS: u8 = 3;

/// `ctx.http.get` carries metadata-scale payloads. Larger or binary bodies use
/// `ctx.http.pipe`, which streams them around the script heap.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// Bounded frames in flight between the upstream reader and the HTTP response.
const PIPE_CHANNEL_CAPACITY: usize = 4;

/// Read size for one pipe frame.
const PIPE_CHUNK_BYTES: usize = 64 * 1024;

/// One upstream call attempt, serialized into the per-request log line.
#[derive(Debug, Clone)]
pub struct CallRecord {
    api: &'static str,
    host: Option<String>,
    path: Option<String>,
    status: Option<u16>,
    response_bytes: Option<u64>,
    duration_ms: Option<f64>,
    redirects: u8,
    error: Option<&'static str>,
    kind: Option<&'static str>,
}

impl CallRecord {
    fn new(api: &'static str) -> Self {
        Self {
            api,
            host: None,
            path: None,
            status: None,
            response_bytes: None,
            duration_ms: None,
            redirects: 0,
            error: None,
            kind: None,
        }
    }

    /// Log shape shared by every upstream call. Query strings never enter it.
    #[must_use]
    pub fn to_json(&self) -> Json {
        json!({
            "api": self.api,
            "host": self.host,
            "path": self.path,
            "status": self.status,
            "response_bytes": self.response_bytes,
            "duration_ms": self.duration_ms,
            "redirects": self.redirects,
            "error": self.error,
            "kind": self.kind,
        })
    }
}

/// Shared per-request upstream call chain. The worker thread appends; the
/// request handler reads it even when a timeout orphans the worker.
pub type CallLog = Arc<Mutex<Vec<CallRecord>>>;

/// Snapshot a request's upstream call chain for the structured log.
#[must_use]
pub fn calls_json(calls: &CallLog) -> Vec<Json> {
    calls.lock().map_or_else(
        |_| Vec::new(),
        |calls| calls.iter().map(CallRecord::to_json).collect(),
    )
}

/// Per-request allowlist and timeout guard for upstream calls.
#[derive(Debug)]
pub struct UpstreamAccess {
    allow_hosts: Vec<String>,
    default_timeout: Duration,
    script_deadline: Instant,
    /// Client `Range` request header; only `ctx.http.pipe` forwards it.
    client_range: Option<String>,
    calls: CallLog,
}

/// HTTP response returned to the script as `{status, headers, text(), bytes()}`.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Streamed body frames for `ctx.http.pipe`; they never enter the script heap.
pub type BodyStream = tokio::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>;

/// How a piped body ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    Complete,
    UpstreamError,
    ClientDisconnected,
}

impl StreamOutcome {
    /// Classify a relay channel that closed before the upstream stream ended.
    ///
    /// hyper drops the response body as soon as the announced
    /// `Content-Length` is satisfied, so a matching byte count is a completed
    /// delivery, not a client disconnect. Without an announced length only an
    /// early client close can close the channel.
    #[must_use]
    pub fn from_channel_close(bytes: u64, announced: Option<u64>) -> Self {
        if announced == Some(bytes) {
            Self::Complete
        } else {
            Self::ClientDisconnected
        }
    }

    /// Request-log error class when the stream did not complete normally.
    #[must_use]
    pub fn error_class(self) -> Option<&'static str> {
        match self {
            Self::Complete => None,
            Self::UpstreamError => Some("upstream_stream_error"),
            Self::ClientDisconnected => Some("client_disconnected"),
        }
    }
}

/// Handle used to finalize one piped call after its body ends. Dropping it
/// without an explicit finish records an abandoned stream, so the call chain
/// never keeps a half-open entry when a script discards a piped response.
#[derive(Debug)]
pub struct StreamCall {
    calls: CallLog,
    index: usize,
    started: Instant,
    finished: AtomicBool,
}

impl StreamCall {
    fn new(calls: CallLog, index: usize, started: Instant) -> Self {
        Self {
            calls,
            index,
            started,
            finished: AtomicBool::new(false),
        }
    }

    /// Finalize this call with the outcome of its body stream.
    pub fn finish(&self, outcome: StreamOutcome, bytes: u64) {
        self.finalize(Some(outcome), bytes);
    }

    fn finalize(&self, outcome: Option<StreamOutcome>, bytes: u64) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        let Some(record) = calls.get_mut(self.index) else {
            return;
        };
        record.duration_ms = Some(elapsed_ms(self.started.elapsed()));
        record.response_bytes = Some(bytes);
        if outcome == Some(StreamOutcome::UpstreamError) {
            record.error = Some("upstream_stream_error");
            record.kind = Some("transport");
        }
    }

    /// Snapshot the call chain after this stream finalized it.
    #[must_use]
    pub fn calls_json(&self) -> Vec<Json> {
        calls_json(&self.calls)
    }
}

impl Drop for StreamCall {
    fn drop(&mut self) {
        self.finalize(None, 0);
    }
}

/// Piped body plus the call handle finalized when the stream ends.
#[derive(Debug)]
pub struct PipeBody {
    pub stream: BodyStream,
    pub call: StreamCall,
}

/// Client response produced by `ctx.http.pipe`: status and headers are decided
/// here, the body streams from the upstream connection as it arrives.
#[derive(Debug)]
pub struct PipeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: PipeBody,
}

/// Category of a transport-layer failure. These stable strings are the only
/// transport detail allowed to reach a client-visible diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Timeout,
    Dns,
    Other,
}

impl TransportKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Dns => "dns",
            Self::Other => "transport",
        }
    }
}

/// Why `ctx.http.get` did not return a response.
#[derive(Debug)]
pub enum Error {
    /// Rejected before the network, or a fail-closed script error.
    Policy(String),
    /// The script passed a URL the host cannot use: it does not parse, or its
    /// scheme is not http/https. Raised only by `ctx.http.pipe`, so a route
    /// script can answer its own invalid-URL business code.
    InvalidUrl(String),
    /// The upstream answered with a redirect chain the host cannot follow:
    /// more than 3 hops, or a Location it cannot resolve. Raised only by
    /// `ctx.http.pipe`; an allowlist rejection stays a policy error.
    Redirect(String),
    /// DNS, connection, TLS, or timeout failure. The message stays
    /// operator-facing; `kind` is the only part that may reach a client.
    Transport {
        kind: TransportKind,
        message: String,
    },
    /// The upstream answered with a final non-2xx status. `ctx.http.pipe`
    /// cannot hand a streaming body to the script, so it raises this
    /// catchable error and the script maps the route's client-visible code.
    Status(u16),
}

impl Error {
    /// Stable class surfaced to the script as `error.code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Policy(_) => "script_error",
            Self::InvalidUrl(_) => "upstream_url_invalid",
            Self::Redirect(_) => "upstream_redirect_error",
            Self::Transport { .. } => "upstream_unreachable",
            Self::Status(_) => "upstream_http_error",
        }
    }

    /// Operator-facing reason; never sent to the client body.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Policy(message)
            | Self::InvalidUrl(message)
            | Self::Redirect(message)
            | Self::Transport { message, .. } => message.clone(),
            Self::Status(status) => format!("ctx.http.pipe: upstream returned status {status}"),
        }
    }

    /// Stable transport kind for logs and `--verbose` detail.
    #[must_use]
    pub fn transport_kind(&self) -> Option<&'static str> {
        match self {
            Self::Transport { kind, .. } => Some(kind.as_str()),
            Self::Policy(_) | Self::InvalidUrl(_) | Self::Redirect(_) | Self::Status(_) => None,
        }
    }
}

impl UpstreamAccess {
    /// Build one request-scoped host from configuration.
    #[must_use]
    pub fn new(
        allow_hosts: Vec<String>,
        timeout: Duration,
        script_deadline: Instant,
        client_range: Option<String>,
        calls: CallLog,
    ) -> Self {
        Self {
            allow_hosts,
            default_timeout: timeout,
            script_deadline,
            client_range,
            calls,
        }
    }

    /// Perform one `ctx.http.get` call from its JSON bridge payload.
    pub fn get(&self, call: &Json) -> Result<Response, Error> {
        let index = self.begin_call("http.get");
        let started = Instant::now();
        let result = self.get_inner(call, index);
        if let Ok(response) = &result {
            self.update_call(index, |record| {
                record.status = Some(response.status);
                record.response_bytes = u64::try_from(response.body.len()).ok();
            });
        }
        self.finish_call(index, started, result.as_ref().err());
        result
    }

    fn get_inner(&self, call: &Json, index: usize) -> Result<Response, Error> {
        let call = self.parse_get_call(call)?;
        let url = Url::parse(&call.url)
            .map_err(|error| Error::Policy(format!("ctx.http.get: invalid URL: {error}")))?;
        self.record_url(index, &url);
        self.validate(&url, Error::Policy)?;

        let deadline = Instant::now() + self.budget(call.timeout)?;
        let response = self.send_following_redirects(
            url,
            deadline,
            None,
            "ctx.http.get",
            Error::Policy,
            index,
        )?;

        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        let body = response
            .into_body()
            .into_with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(map_ureq_error)?;
        Ok(Response {
            status: status.as_u16(),
            headers,
            body,
        })
    }

    /// Perform one `ctx.http.pipe` call from its JSON bridge payload.
    ///
    /// Success means the upstream answered 2xx: the client status and headers
    /// are fixed here and the body is streamed by a background reader. A final
    /// non-2xx answer is a catchable `upstream_http_error`, because a piped
    /// body cannot be inspected by the script.
    pub fn pipe(&self, call: &Json) -> Result<PipeResponse, Error> {
        let index = self.begin_call("http.pipe");
        let started = Instant::now();
        let result = self.pipe_inner(call, index, started);
        if let Err(error) = &result {
            self.finish_call(index, started, Some(error));
        }
        result
    }

    fn pipe_inner(
        &self,
        call: &Json,
        index: usize,
        started: Instant,
    ) -> Result<PipeResponse, Error> {
        let call = self.parse_pipe_call(call)?;
        let url = Url::parse(&call.url)
            .map_err(|error| Error::InvalidUrl(format!("ctx.http.pipe: invalid URL: {error}")))?;
        self.record_url(index, &url);
        self.validate(&url, Error::InvalidUrl)?;
        // `ctx.http.pipe` keeps the configured upstream timeout; its opts are
        // (status, headers) only, so a pipe borrows the whole remaining budget.
        let deadline = Instant::now() + self.budget(None)?;
        let response = self.send_following_redirects(
            url,
            deadline,
            self.client_range.as_deref(),
            "ctx.http.pipe",
            Error::Redirect,
            index,
        )?;
        let upstream_status = response.status().as_u16();
        self.update_call(index, |record| record.status = Some(upstream_status));
        if !(200..=299).contains(&upstream_status) {
            return Err(Error::Status(upstream_status));
        }

        // The script owns Content-Type and Content-Disposition; the upstream
        // Range contract travels with the stream. The upstream 2xx status is
        // the default, so a Range 206 keeps its partial-response semantics.
        let client_status = call.status;
        let mut headers = call.headers;
        for (name, value) in response.headers() {
            let name = name.as_str().to_ascii_lowercase();
            let passthrough = name == "content-range" || name == "content-length";
            let already_set = headers
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(&name));
            if passthrough && !already_set {
                if let Ok(value) = value.to_str() {
                    headers.push((name, value.to_string()));
                }
            }
        }
        let status = client_status.unwrap_or(upstream_status);
        let (sender, stream) = tokio::sync::mpsc::channel(PIPE_CHANNEL_CAPACITY);
        let reader = response.into_body().into_reader();
        tokio::task::spawn_blocking(move || pump_body(reader, sender));
        Ok(PipeResponse {
            status,
            headers,
            body: PipeBody {
                stream,
                call: StreamCall::new(self.calls.clone(), index, started),
            },
        })
    }

    fn begin_call(&self, api: &'static str) -> usize {
        let Ok(mut calls) = self.calls.lock() else {
            return usize::MAX;
        };
        calls.push(CallRecord::new(api));
        calls.len() - 1
    }

    fn update_call(&self, index: usize, update: impl FnOnce(&mut CallRecord)) {
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        if let Some(record) = calls.get_mut(index) {
            update(record);
        }
    }

    fn finish_call(&self, index: usize, started: Instant, error: Option<&Error>) {
        self.update_call(index, |record| {
            record.duration_ms = Some(elapsed_ms(started.elapsed()));
            if let Some(error) = error {
                record.error = Some(error.code());
                record.kind = error.transport_kind();
                if let Error::Status(status) = error {
                    record.status = Some(*status);
                }
            }
        });
    }

    fn record_url(&self, index: usize, url: &Url) {
        let host = url.host_str().map(str::to_string);
        let path = Some(url.path().to_string());
        self.update_call(index, |record| {
            record.host = host;
            record.path = path;
        });
    }

    /// Effective upstream budget for one call: the smaller of the remaining
    /// script time and the requested timeout, less a reply margin.
    ///
    /// tradeoff: when the upstream timeout would consume the whole script
    /// budget, the call is cut slightly early so the transport error can
    /// surface as a catchable failure instead of racing the outer script
    /// timeout. Ceiling: upgrade when Boa exposes an in-engine interrupt hook.
    fn budget(&self, requested: Option<Duration>) -> Result<Duration, Error> {
        let remaining = self
            .script_deadline
            .saturating_duration_since(Instant::now());
        let requested = requested.unwrap_or(self.default_timeout);
        let budget = if requested >= remaining {
            remaining.saturating_sub(reply_margin(remaining))
        } else {
            requested
        };
        if budget.is_zero() {
            return Err(timeout_error());
        }
        Ok(budget)
    }

    /// Send one GET and follow up to `MAX_REDIRECTS` redirects. Every hop is
    /// re-validated against the allowlist; `redirect_error` classifies a chain
    /// the caller cannot follow, while an allowlist rejection stays a policy
    /// error and never masquerades as an upstream failure.
    fn send_following_redirects(
        &self,
        mut url: Url,
        deadline: Instant,
        range: Option<&str>,
        call: &str,
        redirect_error: fn(String) -> Error,
        index: usize,
    ) -> Result<ureq::http::Response<ureq::Body>, Error> {
        let mut redirects = 0_u8;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timeout_error());
            }
            let response = self.fetch(&url, left, range)?;
            if is_redirect(response.status().as_u16()) {
                if let Some(location) = response.headers().get(ureq::http::header::LOCATION) {
                    if redirects >= MAX_REDIRECTS {
                        return Err(redirect_error(format!(
                            "{call}: redirect limit exceeded (max 3 hops)"
                        )));
                    }
                    let location = location.to_str().map_err(|_| {
                        redirect_error(format!("{call}: redirect Location is not a valid string"))
                    })?;
                    let next = url.join(location).map_err(|error| {
                        redirect_error(format!("{call}: invalid redirect Location: {error}"))
                    })?;
                    self.validate(&next, redirect_error)?;
                    url = next;
                    redirects += 1;
                    self.update_call(index, |record| record.redirects = redirects);
                    self.record_url(index, &url);
                    continue;
                }
            }
            return Ok(response);
        }
    }

    /// Send one GET, optionally carrying the client's `Range` header.
    /// Redirects stay disabled at the client; callers re-validate every hop.
    fn fetch(
        &self,
        url: &Url,
        timeout: Duration,
        range: Option<&str>,
    ) -> Result<ureq::http::Response<ureq::Body>, Error> {
        let mut request = agent()
            .get(url.as_str())
            .config()
            .timeout_global(Some(timeout))
            .build();
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        request.call().map_err(map_ureq_error)
    }

    /// Parse and fail-closed validate the bridge payload.
    fn parse_get_call(&self, call: &Json) -> Result<GetCall, Error> {
        let Some(object) = call.as_object() else {
            return Err(Error::Policy(
                "ctx.http.get: call payload must be an object".into(),
            ));
        };
        for key in object.keys() {
            if key != "url" && key != "timeout_ms" {
                return Err(Error::Policy(format!(
                    "ctx.http.get: unknown opts key {key:?}"
                )));
            }
        }
        let Some(url) = object.get("url").and_then(Json::as_str) else {
            return Err(Error::Policy("ctx.http.get: url must be a string".into()));
        };
        let timeout = match object.get("timeout_ms") {
            None => None,
            Some(value) => {
                let ms = value.as_u64().filter(|ms| *ms > 0).ok_or_else(|| {
                    Error::Policy("ctx.http.get: opts.timeout_ms must be a positive integer".into())
                })?;
                Some(Duration::from_millis(ms))
            }
        };
        Ok(GetCall {
            url: url.to_string(),
            timeout,
        })
    }

    /// Parse and fail-closed validate the `ctx.http.pipe` payload. The JS
    /// prelude already checks shapes; this is the independent host-side guard.
    fn parse_pipe_call(&self, call: &Json) -> Result<PipeCall, Error> {
        let Some(object) = call.as_object() else {
            return Err(Error::Policy(
                "ctx.http.pipe: call payload must be an object".into(),
            ));
        };
        for key in object.keys() {
            if !matches!(key.as_str(), "url" | "status" | "headers") {
                return Err(Error::Policy(format!(
                    "ctx.http.pipe: unknown opts key {key:?}"
                )));
            }
        }
        let Some(url) = object.get("url").and_then(Json::as_str) else {
            return Err(Error::Policy("ctx.http.pipe: url must be a string".into()));
        };
        let status = match object.get("status") {
            None | Some(Json::Null) => None,
            Some(value) => {
                let status = value
                    .as_u64()
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|status| (100..=599).contains(status))
                    .ok_or_else(|| {
                        Error::Policy(
                            "ctx.http.pipe: status must be an integer in [100, 599]".into(),
                        )
                    })?;
                Some(status)
            }
        };
        let headers = match object.get("headers") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(rows)) => {
                let mut headers = Vec::with_capacity(rows.len());
                for row in rows {
                    let pair = row
                        .as_array()
                        .filter(|pair| pair.len() == 2)
                        .ok_or_else(|| {
                            Error::Policy(
                                "ctx.http.pipe: headers must be [name, value] pairs".into(),
                            )
                        })?;
                    let (Some(name), Some(value)) = (
                        pair.first().and_then(Json::as_str),
                        pair.get(1).and_then(Json::as_str),
                    ) else {
                        return Err(Error::Policy(
                            "ctx.http.pipe: headers must be [name, value] pairs".into(),
                        ));
                    };
                    headers.push((name.to_string(), value.to_string()));
                }
                headers
            }
            Some(_) => {
                return Err(Error::Policy(
                    "ctx.http.pipe: headers must be [name, value] pairs".into(),
                ));
            }
        };
        Ok(PipeCall {
            url: url.to_string(),
            status,
            headers,
        })
    }

    /// Protocol + host allowlist validation. Port is deliberately not matched.
    /// `invalid_url` classifies a scheme rejection for the calling API; an
    /// allowlist rejection is always a policy error, never an invalid URL.
    fn validate(&self, url: &Url, invalid_url: fn(String) -> Error) -> Result<(), Error> {
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(invalid_url(format!(
                    "upstream protocol {other:?} is not allowed (use http or https)"
                )));
            }
        }
        let host = match url.host() {
            Some(UrlHost::Domain(domain)) => domain.to_string(),
            Some(UrlHost::Ipv4(ip)) => ip.to_string(),
            Some(UrlHost::Ipv6(ip)) => ip.to_string(),
            None => return Err(Error::Policy("ctx.http.get: URL has no host".into())),
        };
        if self
            .allow_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&host))
        {
            return Ok(());
        }
        Err(Error::Policy(format!(
            "upstream host {host:?} is not in upstream.allow_hosts"
        )))
    }
}

/// Parsed bridge payload; unknown keys are rejected before this point.
struct GetCall {
    url: String,
    timeout: Option<Duration>,
}

/// Parsed `ctx.http.pipe` payload: the client status and headers are fixed
/// before the upstream body is handed to the HTTP layer.
struct PipeCall {
    url: String,
    status: Option<u16>,
    headers: Vec<(String, String)>,
}

/// Time left for the script itself to catch and map a transport error.
fn reply_margin(remaining: Duration) -> Duration {
    std::cmp::min(remaining / 10, Duration::from_millis(100))
}

fn elapsed_ms(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

fn timeout_error() -> Error {
    Error::Transport {
        kind: TransportKind::Timeout,
        message: "ctx.http: upstream timeout".into(),
    }
}

/// Move upstream bytes into the response channel until the body ends, fails,
/// or the client drops the response. `blocking_send` provides backpressure, so
/// a slow client cannot make the host buffer a whole file. The sender is taken
/// by value because the reader owns it for the lifetime of the spawned task.
#[allow(clippy::needless_pass_by_value)]
fn pump_body(
    mut reader: impl Read,
    sender: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) {
    let mut buffer = vec![0_u8; PIPE_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if sender.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.blocking_send(Err(error));
                break;
            }
        }
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

// One lazily built agent per blocking worker thread; connections are reused
// within the thread but no state is visible across requests.
thread_local! {
    static AGENT: OnceCell<ureq::Agent> = const { OnceCell::new() };
}

fn agent() -> ureq::Agent {
    AGENT.with(|cell| {
        cell.get_or_init(|| {
            // Platform roots + the platform verifier keep TLS trust anchored to
            // the host store (and to enterprise proxies) without bundling a
            // separately licensed root data set. Ring is the crypto provider.
            let crypto = std::sync::Arc::new(rustls::crypto::ring::default_provider());
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .proxy(None)
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .provider(ureq::tls::TlsProvider::Rustls)
                        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                        .unversioned_rustls_crypto_provider(crypto)
                        .build(),
                )
                .build()
                .new_agent()
        })
        .clone()
    })
}

/// 4xx/5xx are data (ADR 0005); everything else is a transport failure.
fn map_ureq_error(error: ureq::Error) -> Error {
    match error {
        ureq::Error::BadUri(message) => Error::Policy(format!("ctx.http: invalid URL: {message}")),
        ureq::Error::Http(error) => Error::Policy(format!("ctx.http: invalid request: {error}")),
        ureq::Error::Timeout(_) => timeout_error(),
        ureq::Error::HostNotFound => Error::Transport {
            kind: TransportKind::Dns,
            message: "ctx.http: upstream host not found".into(),
        },
        ureq::Error::BodyExceedsLimit(limit) => Error::Policy(format!(
            "ctx.http.get: upstream response body exceeds the {limit}-byte limit"
        )),
        other => Error::Transport {
            kind: TransportKind::Other,
            message: format!("ctx.http: {other}"),
        },
    }
}

/// Serialize one response for the JS bridge.
///
/// Bodies cross as base64 (about 1.33x) instead of a JSON number array
/// (roughly 3-4x); `bytes()` decodes into a `Uint8Array` only when called.
/// T6's `ctx.http.pipe` streams large bodies around the script heap entirely.
#[must_use]
pub fn response_json(response: Response) -> Json {
    let headers = response
        .headers
        .into_iter()
        .map(|(name, value)| (name, Json::String(value)))
        .collect::<serde_json::Map<String, Json>>();
    json!({
        "status": response.status,
        "headers": headers,
        "text": String::from_utf8_lossy(&response.body),
        "body_base64": BASE64.encode(&response.body),
    })
}

#[cfg(test)]
mod tests {
    use super::StreamOutcome;

    #[test]
    fn channel_close_after_the_announced_length_completes() {
        assert_eq!(
            StreamOutcome::from_channel_close(13, Some(13)),
            StreamOutcome::Complete
        );
    }

    #[test]
    fn channel_close_before_the_announced_length_is_a_disconnect() {
        assert_eq!(
            StreamOutcome::from_channel_close(5, Some(13)),
            StreamOutcome::ClientDisconnected
        );
    }

    #[test]
    fn channel_close_without_an_announced_length_is_a_disconnect() {
        assert_eq!(
            StreamOutcome::from_channel_close(13, None),
            StreamOutcome::ClientDisconnected
        );
    }
}
