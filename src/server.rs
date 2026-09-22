// HTTP surface: transport in, match, script run, response out.
// Client error classes per docs/contracts/cli.md: not_found, script_error,
// script_no_response. Stream failures are log-only classes.
use crate::config::Config;
use crate::matcher::match_route;
use crate::script::{self, RequestSnapshot, ResponseBody, ScriptResponse};
use crate::upstream;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::CONTENT_LENGTH;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_stream::wrappers::ReceiverStream;

/// One matched (or unmatched) request, before it is mapped onto HTTP.
struct Routed {
    route_label: String,
    params: HashMap<String, String>,
    handled: Handled,
    script_logs: Vec<script::LogRecord>,
    upstream_calls: Vec<Value>,
    script_duration_ms: Option<f64>,
}

pub struct AppState {
    config: Config,
    host: String,
    verbose: bool,
    sequence: AtomicU64,
}

impl AppState {
    fn new(config: Config, addr: SocketAddr, verbose: bool) -> Self {
        let host = addr.to_string();
        Self {
            config,
            host,
            verbose,
            sequence: AtomicU64::new(0),
        }
    }

    /// Correlation id: not a security token, just for joining logs/errors.
    #[allow(clippy::cast_possible_truncation)]
    fn request_id(&self) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default();
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        format!("{nanos:016x}{seq:08x}")
    }

    /// Match one request and run its route script, if any.
    async fn route_request(
        &self,
        method: &Method,
        method_label: &str,
        path: &str,
        uri: &Uri,
        headers: &HeaderMap,
        body: &Bytes,
    ) -> Routed {
        match match_route(&self.config.routes, method_label, path) {
            None => Routed {
                route_label: String::new(),
                params: HashMap::new(),
                handled: Handled::NotFound,
                script_logs: Vec::new(),
                upstream_calls: Vec::new(),
                script_duration_ms: None,
            },
            Some(found) => {
                let route = &self.config.routes[found.route_index];
                let label = route.name.clone().unwrap_or_else(|| route.path.clone());
                let snapshot = RequestSnapshot::new(
                    method,
                    path,
                    found.params.clone(),
                    uri.query().unwrap_or(""),
                    headers,
                    body,
                );
                let script_started = Instant::now();
                let outcome = match std::fs::read_to_string(&route.script) {
                    Ok(source) => {
                        let timeout = Duration::from_millis(self.config.sandbox.script_timeout_ms);
                        script::execute(source, snapshot, timeout, self.config.upstream.clone())
                            .await
                    }
                    // The path was validated at load time; losing the file now
                    // is a runtime failure, not a silent 404.
                    Err(error) => script::Outcome::failed(script::Error::Failed(format!(
                        "cannot read script {}: {error}",
                        route.script.display()
                    ))),
                };
                let script_duration_ms = Some(elapsed_ms(script_started));
                let script::Outcome {
                    response,
                    logs,
                    error,
                    upstream_calls,
                } = outcome;
                let handled = match (response, error) {
                    (Some(response), _) => Handled::Responded(response),
                    (None, Some(error)) => Handled::Failed(error),
                    (None, None) => Handled::Failed(script::Error::NoResponse),
                };
                Routed {
                    route_label: label,
                    params: found.params,
                    handled,
                    script_logs: logs,
                    upstream_calls,
                    script_duration_ms,
                }
            }
        }
    }
}

/// Outcome of one matched route, before it is mapped onto HTTP.
enum Handled {
    /// No route matched method + path.
    NotFound,
    /// The script did not produce a response.
    Failed(script::Error),
    /// The script called `ctx.respond`.
    Responded(ScriptResponse),
}

/// Bind address from configuration. The loader validates the IP literal, and
/// this second parse keeps the library entry point safe when `Config` is built
/// directly.
pub fn bind_address(config: &Config) -> io::Result<SocketAddr> {
    let ip = crate::config::parse_bind(&config.server.bind).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "server.bind must be an IP address, got {:?}",
                config.server.bind
            ),
        )
    })?;
    Ok(SocketAddr::new(ip, config.server.port))
}

pub async fn run(config: Config, addr: SocketAddr, verbose: bool) -> io::Result<()> {
    // Tradeoff: only log "listening" after socket binds successfully.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    eprintln!("stuntdouble listening on http://{bound}");
    let state = Arc::new(AppState::new(config, bound, verbose));
    let app = Router::new().fallback(any(handle)).with_state(state);
    axum::serve(listener, app).await
}

async fn handle(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    let request_id = state.request_id();
    let method_label = method.as_str().to_ascii_uppercase();
    let path = uri.path().to_string();
    // Client-supplied id is recorded but never adopted or forwarded upstream.
    let client_request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let request_header_log = loggable_headers(&headers, &REQUEST_LOG_HEADERS);
    let request_body_bytes = body.len();

    let Routed {
        route_label,
        params,
        handled,
        script_logs,
        upstream_calls,
        script_duration_ms,
    } = state
        .route_request(&method, &method_label, &path, &uri, &headers, &body)
        .await;

    let mapped = map_handled(handled, &request_id, state.verbose);

    let response_header_log = loggable_headers(&mapped.headers, &RESPONSE_LOG_HEADERS);
    let mut payload = json!({
        "request_id": &request_id,
        "route": route_label,
        "method": method_label,
        "path": path,
        "status": mapped.status.as_u16(),
        "error": mapped.error_class,
        "elapsed_ms": elapsed_ms(started),
        "script_duration_ms": script_duration_ms,
        "request_body_bytes": request_body_bytes,
        "response_body_bytes": mapped.body_bytes,
        "request_headers": request_header_log,
        "response_headers": response_header_log,
        "upstream_calls": upstream_calls,
        "params": params,
        "client_request_id": client_request_id,
        "host": &state.host,
    });
    if !script_logs.is_empty() {
        // `ctx.log.*` stays in this one server-side record, never in a body.
        payload["script_logs"] = json!(script_logs
            .iter()
            .map(|record| json!({ "level": record.level, "message": record.message }))
            .collect::<Vec<_>>());
    }
    let Mapped {
        status,
        headers,
        body,
        ..
    } = mapped;
    let mut headers = headers;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        headers.insert(HeaderName::from_static("x-request-id"), value);
    }
    let body = match body {
        MappedBody::Ready(body) => {
            log(&payload);
            body
        }
        MappedBody::Stream(pipe) => {
            // hyper frames the streamed body with this length and drops the
            // relay as soon as it is satisfied, so a late channel close must
            // not be misread as a client disconnect.
            let announced = headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            stream_body(pipe, payload, started, announced)
        }
    };
    (status, headers, body).into_response()
}

/// Script response body, either buffered and ready or still streaming.
enum MappedBody {
    Ready(Body),
    Stream(upstream::PipeBody),
}

/// One handled route mapped onto the HTTP response plus its log-only fields.
struct Mapped {
    status: StatusCode,
    error_class: &'static str,
    headers: HeaderMap,
    body: MappedBody,
    body_bytes: Option<u64>,
}

fn map_handled(handled: Handled, request_id: &str, verbose: bool) -> Mapped {
    match handled {
        Handled::NotFound => {
            let body = error_body(request_id, "not_found", None);
            Mapped {
                status: StatusCode::NOT_FOUND,
                error_class: "not_found",
                headers: HeaderMap::new(),
                body_bytes: u64::try_from(body.len()).ok(),
                body: MappedBody::Ready(Body::from(body)),
            }
        }
        Handled::Failed(error) => failed_mapped(&error, request_id, verbose),
        Handled::Responded(response) => match response_headers(&response.headers) {
            Ok(headers) => Mapped {
                status: StatusCode::from_u16(response.status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                error_class: "",
                headers,
                body_bytes: script_body_size(&response),
                body: match response.body {
                    ResponseBody::Text(text) => MappedBody::Ready(Body::from(text.into_bytes())),
                    ResponseBody::Bytes(bytes) => MappedBody::Ready(Body::from(bytes)),
                    ResponseBody::Stream(pipe) => MappedBody::Stream(pipe),
                },
            },
            Err(message) => failed_mapped(&script::Error::Failed(message), request_id, verbose),
        },
    }
}

/// Map a script failure onto its client-visible status, body, and log class.
fn failed_mapped(error: &script::Error, request_id: &str, verbose: bool) -> Mapped {
    let class = error.class();
    let client_detail = if verbose { Some(error.detail()) } else { None };
    let body = error_body(request_id, class, client_detail);
    Mapped {
        status: error.status(),
        error_class: class,
        headers: HeaderMap::new(),
        body_bytes: u64::try_from(body.len()).ok(),
        body: MappedBody::Ready(Body::from(body)),
    }
}

/// Request headers copied into the per-request log. Tokens and cookies are
/// deliberately absent.
const REQUEST_LOG_HEADERS: [&str; 5] = [
    "accept",
    "content-type",
    "content-length",
    "range",
    "user-agent",
];

/// Response headers copied into the per-request log.
const RESPONSE_LOG_HEADERS: [&str; 3] = ["content-type", "content-length", "content-range"];

/// Bounded frames between the stream relay and the HTTP response body.
const STREAM_CHANNEL_CAPACITY: usize = 4;

fn loggable_headers(headers: &HeaderMap, allowlist: &[&str]) -> Map<String, Value> {
    let mut logged = Map::new();
    for name in allowlist {
        let values = headers
            .get_all(*name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            logged.insert((*name).to_string(), json!(values.join(", ")));
        }
    }
    logged
}

/// Buffered body length. A streamed body is counted by the relay that writes
/// its completion log, so its payload starts from `null`.
fn script_body_size(response: &ScriptResponse) -> Option<u64> {
    match &response.body {
        ResponseBody::Text(text) => u64::try_from(text.len()).ok(),
        ResponseBody::Bytes(bytes) => u64::try_from(bytes.len()).ok(),
        ResponseBody::Stream(_) => None,
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    let millis = started.elapsed().as_secs_f64() * 1000.0;
    (millis * 100.0).round() / 100.0
}

/// Relay a piped body to the client and write the request log when it ends.
/// The status line is already on the wire, so a mid-stream failure is recorded
/// in the log without changing the client-visible status.
fn stream_body(
    pipe: upstream::PipeBody,
    payload: Value,
    started: Instant,
    announced: Option<u64>,
) -> Body {
    let upstream::PipeBody { stream, call } = pipe;
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        relay_stream(stream, call, sender, payload, started, announced).await;
    });
    Body::from_stream(ReceiverStream::new(receiver))
}

async fn relay_stream(
    mut stream: upstream::BodyStream,
    call: upstream::StreamCall,
    sender: tokio::sync::mpsc::Sender<Result<Bytes, io::Error>>,
    mut payload: Value,
    started: Instant,
    announced: Option<u64>,
) {
    let mut bytes = 0_u64;
    let mut outcome = upstream::StreamOutcome::Complete;
    let mut terminal_error = None;
    loop {
        tokio::select! {
            item = stream.recv() => match item {
                Some(Ok(chunk)) => {
                    let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
                    if sender.send(Ok(Bytes::from(chunk))).await.is_err() {
                        outcome = upstream::StreamOutcome::from_channel_close(bytes, announced);
                        break;
                    }
                    bytes = bytes.saturating_add(chunk_bytes);
                }
                Some(Err(error)) => {
                    outcome = upstream::StreamOutcome::UpstreamError;
                    terminal_error = Some(error);
                    break;
                }
                None => break,
            },
            () = sender.closed() => {
                outcome = upstream::StreamOutcome::from_channel_close(bytes, announced);
                break;
            }
        }
    }
    call.finish(outcome, bytes);
    payload["response_body_bytes"] = json!(bytes);
    payload["elapsed_ms"] = json!(elapsed_ms(started));
    payload["upstream_calls"] = json!(call.calls_json());
    if let Some(class) = outcome.error_class() {
        payload["error"] = json!(class);
    }
    log(&payload);
    if let Some(error) = terminal_error {
        let _ = sender.send(Err(error)).await;
    }
}

/// JSON error body shared by every non-script response.
fn error_body(request_id: &str, class: &str, detail: Option<&str>) -> Vec<u8> {
    let mut body = json!({ "request_id": request_id, "error": class });
    if let Some(detail) = detail {
        body["detail"] = json!(detail);
    }
    serde_json::to_vec(&body).unwrap_or_default()
}

/// Convert script-provided header pairs. The script boundary has already
/// validated the same grammar; this second check keeps internal state changes
/// from bypassing the fail-closed contract.
fn response_headers(pairs: &[(String, String)]) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in script::validated_header_pairs(pairs)? {
        headers.append(name, value);
    }
    Ok(headers)
}

fn log(payload: &serde_json::Value) {
    if let Ok(line) = serde_json::to_string(payload) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "{line}");
    }
}
