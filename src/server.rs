// HTTP surface: transport in, match, engine response. Matched route answers 501.
use crate::config::Config;
use crate::matcher::match_route;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
) -> Response {
    let started = Instant::now();
    let request_id = state.request_id();
    let method = method.as_str().to_ascii_uppercase();
    let path = uri.path().to_string();
    // Client-supplied id is recorded but never adopted/forwarded.
    let client_request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (status, error_class, route_label, params) =
        match match_route(&state.config.routes, &method, &path) {
            Some(found) => {
                let route = &state.config.routes[found.route_index];
                let label = route.name.clone().unwrap_or_else(|| route.path.clone());
                (
                    StatusCode::NOT_IMPLEMENTED,
                    "script_unimplemented",
                    label,
                    found.params,
                )
            }
            None => (
                StatusCode::NOT_FOUND,
                "not_found",
                String::new(),
                HashMap::new(),
            ),
        };

    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let payload = json!({
        "request_id": request_id,
        "route": route_label,
        "method": method,
        "path": path,
        "status": status.as_u16(),
        "error": error_class,
        "elapsed_ms": (elapsed_ms * 100.0).round() / 100.0,
        "params": params,
        "client_request_id": client_request_id,
        "host": state.host.clone(),
    });
    log(&payload);

    let body = json!({ "request_id": request_id, "error": error_class });
    (
        status,
        [(
            axum::http::header::HeaderName::from_static("x-request-id"),
            request_id,
        )],
        axum::Json(body),
    )
        .into_response()
}

fn log(payload: &serde_json::Value) {
    if let Ok(line) = serde_json::to_string(payload) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "{line}");
    }
}
