//! Allowlisted upstream HTTP calls for `ctx.http.get`.
//! Contract: docs/contracts/ctx-api.md
//!
//! The script host is synchronous, so this module uses ureq's blocking client.
//! Redirects are disabled at the client and followed here, one hop at a time,
//! so every target is re-validated against the allowlist before it is fetched.
use std::cell::OnceCell;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value as Json};
use url::{Host as UrlHost, Url};

/// Maximum redirects followed after the initial request.
const MAX_REDIRECTS: u8 = 3;

/// `ctx.http.get` carries metadata-scale payloads. Larger or binary bodies use
/// the streaming pipe API (T6); this cap keeps the JavaScript bridge within the
/// script memory budget.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// Per-request allowlist and timeout guard for upstream calls.
#[derive(Debug)]
pub struct UpstreamAccess {
    allow_hosts: Vec<String>,
    default_timeout: Duration,
    script_deadline: Instant,
}

/// HTTP response returned to the script as `{status, headers, text(), bytes()}`.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Why `ctx.http.get` did not return a response.
#[derive(Debug)]
pub enum Error {
    /// Rejected before the network, or a fail-closed script error.
    Policy(String),
    /// DNS, connection, TLS, or timeout failure.
    Transport(String),
}

impl Error {
    /// Stable class surfaced to the script as `error.code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Policy(_) => "script_error",
            Self::Transport(_) => "upstream_unreachable",
        }
    }

    /// Operator-facing reason; never sent to the client body.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Policy(message) | Self::Transport(message) => message,
        }
    }
}

impl UpstreamAccess {
    /// Build one request-scoped host from configuration.
    #[must_use]
    pub fn new(allow_hosts: Vec<String>, timeout: Duration, script_deadline: Instant) -> Self {
        Self {
            allow_hosts,
            default_timeout: timeout,
            script_deadline,
        }
    }

    /// Perform one `ctx.http.get` call from its JSON bridge payload.
    pub fn get(&self, call: &Json) -> Result<Response, Error> {
        let call = self.parse_get_call(call)?;
        let mut url = Url::parse(&call.url)
            .map_err(|error| Error::Policy(format!("ctx.http.get: invalid URL: {error}")))?;
        self.validate(&url)?;

        let remaining = self
            .script_deadline
            .saturating_duration_since(Instant::now());
        let requested = call.timeout.unwrap_or(self.default_timeout);
        // tradeoff: when the upstream timeout would consume the whole script
        // budget, reserve a small margin so the transport error can surface as
        // 502 upstream_unreachable instead of racing the outer script timeout.
        // Ceiling: a slow upstream is cut slightly earlier than requested;
        // upgrade when Boa exposes an in-engine interrupt hook.
        let budget = if requested >= remaining {
            remaining.saturating_sub(reply_margin(remaining))
        } else {
            requested
        };
        if budget.is_zero() {
            return Err(timeout_error());
        }
        let deadline = Instant::now() + budget;
        let agent = agent();
        let mut redirects = 0_u8;

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timeout_error());
            }
            let response = agent
                .get(url.as_str())
                .config()
                .timeout_global(Some(left))
                .build()
                .call()
                .map_err(map_ureq_error)?;

            let status = response.status();
            if is_redirect(status.as_u16()) {
                if let Some(location) = response.headers().get(ureq::http::header::LOCATION) {
                    if redirects >= MAX_REDIRECTS {
                        return Err(Error::Policy(
                            "ctx.http.get: redirect limit exceeded (max 3 hops)".into(),
                        ));
                    }
                    let location = location.to_str().map_err(|_| {
                        Error::Policy(
                            "ctx.http.get: redirect Location is not a valid string".into(),
                        )
                    })?;
                    let next = url.join(location).map_err(|error| {
                        Error::Policy(format!("ctx.http.get: invalid redirect Location: {error}"))
                    })?;
                    self.validate(&next)?;
                    url = next;
                    redirects += 1;
                    continue;
                }
            }

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
            return Ok(Response {
                status: status.as_u16(),
                headers,
                body,
            });
        }
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

    /// Protocol + host allowlist validation. Port is deliberately not matched.
    fn validate(&self, url: &Url) -> Result<(), Error> {
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(Error::Policy(format!(
                    "ctx.http.get: protocol {other:?} is not allowed (use http or https)"
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
            "ctx.http.get: host {host:?} is not in upstream.allow_hosts"
        )))
    }
}

/// Parsed bridge payload; unknown keys are rejected before this point.
struct GetCall {
    url: String,
    timeout: Option<Duration>,
}

/// Time left for the script itself to catch and map a transport error.
fn reply_margin(remaining: Duration) -> Duration {
    std::cmp::min(remaining / 10, Duration::from_millis(100))
}

fn timeout_error() -> Error {
    Error::Transport("ctx.http.get: upstream timeout".into())
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
        ureq::Error::BadUri(message) => {
            Error::Policy(format!("ctx.http.get: invalid URL: {message}"))
        }
        ureq::Error::Http(error) => {
            Error::Policy(format!("ctx.http.get: invalid request: {error}"))
        }
        ureq::Error::Timeout(_) => timeout_error(),
        ureq::Error::HostNotFound => {
            Error::Transport("ctx.http.get: upstream host not found".into())
        }
        ureq::Error::BodyExceedsLimit(limit) => Error::Policy(format!(
            "ctx.http.get: upstream response body exceeds the {limit}-byte limit"
        )),
        other => Error::Transport(format!("ctx.http.get: {other}")),
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
