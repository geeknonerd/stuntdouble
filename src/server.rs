// HTTP surface: transport in, match, script run, response out.
// Error classes per docs/contracts/cli.md: not_found, script_error,
// script_no_response.
use crate::config::Config;
use crate::matcher::match_route;
use crate::script::{self, RequestSnapshot, ScriptResponse};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct AppState {
    config: Config,
    host: String,
    sequence: AtomicU64,
}

impl AppState {
    fn new(config: Config, addr: SocketAddr) -> Self {
        let host = addr.to_string();
        Self {
            config,
            host,
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

/// Bind address from configuration. Only numeric IP accepted in T1.
pub fn bind_address(config: &Config) -> io::Result<SocketAddr> {
    let ip: std::net::IpAddr = config.server.bind.parse().map_err(|_| {
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

pub async fn run(config: Config, addr: SocketAddr) -> io::Result<()> {
    // Tradeoff: only log "listening" after socket binds successfully.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    eprintln!("stuntdouble listening on http://{bound}");
    let state = Arc::new(AppState::new(config, bound));
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

    let (route_label, params, handled, script_logs) =
        match match_route(&state.config.routes, &method_label, &path) {
            None => (String::new(), HashMap::new(), Handled::NotFound, Vec::new()),
            Some(found) => {
                let route = &state.config.routes[found.route_index];
                let label = route.name.clone().unwrap_or_else(|| route.path.clone());
                let snapshot = RequestSnapshot::new(
                    &method,
                    &path,
                    found.params.clone(),
                    uri.query().unwrap_or(""),
                    &headers,
                    &body,
                );
                let outcome = match std::fs::read_to_string(&route.script) {
                    Ok(source) => {
                        let timeout = Duration::from_millis(state.config.sandbox.script_timeout_ms);
                        script::execute(source, snapshot, timeout).await
                    }
                    // The path was validated at load time; losing the file now
                    // is a runtime failure, not a silent 404.
                    Err(error) => script::Outcome::failed(script::Error::Failed(format!(
                        "cannot read script {}: {error}",
                        route.script.display()
                    ))),
                };
                let handled = match (outcome.response, outcome.error) {
                    (Some(response), _) => Handled::Responded(response),
                    (None, Some(error)) => Handled::Failed(error),
                    (None, None) => Handled::Failed(script::Error::NoResponse),
                };
                (label, found.params, handled, outcome.logs)
            }
        };

    let (status, error_class, response_headers, body_bytes) = match handled {
        Handled::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            HeaderMap::new(),
            error_body(&request_id, "not_found"),
        ),
        Handled::Failed(error) => {
            let class = error.class();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                class,
                HeaderMap::new(),
                error_body(&request_id, class),
            )
        }
        Handled::Responded(response) => (
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            "",
            response_headers(&response.headers),
            response.body.into_bytes(),
        ),
    };

    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut payload = json!({
        "request_id": &request_id,
        "route": route_label,
        "method": method_label,
        "path": path,
        "status": status.as_u16(),
        "error": error_class,
        "elapsed_ms": (elapsed_ms * 100.0).round() / 100.0,
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
    log(&payload);

    let mut headers = response_headers;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        headers.insert(HeaderName::from_static("x-request-id"), value);
    }
    (status, headers, Body::from(body_bytes)).into_response()
}

/// JSON error body shared by every non-script response.
fn error_body(request_id: &str, class: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "request_id": request_id, "error": class })).unwrap_or_default()
}

/// Convert script-provided header pairs; malformed entries are dropped.
fn response_headers(pairs: &[(String, String)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) else {
            continue;
        };
        headers.append(name, value);
    }
    headers
}

fn log(payload: &serde_json::Value) {
    if let Ok(line) = serde_json::to_string(payload) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "{line}");
    }
}
