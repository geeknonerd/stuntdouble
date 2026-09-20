//! End-to-end checks through the only seam: the built binary plus real HTTP.
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_stuntdouble");

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn good_config() -> &'static str {
    r#"config_version = "1"

[server]
bind = "127.0.0.1"
port = {port}

[files]
root = "./files"

[[routes]]
name = "manifest"
method = "GET"
path = "/demo/documents/manifest/:group"
script = "scripts/manifest.js"
"#
}

/// Script every fixture route runs unless a test supplies its own.
const OK_SCRIPT: &str = r#"ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, "ok");
"#;

/// Write a config plus the directories it references. Returns the config path.
fn fixture(dir: &Path, port: u16, body: &str) -> PathBuf {
    fixture_with_script(dir, port, body, OK_SCRIPT)
}

fn fixture_with_script(dir: &Path, port: u16, body: &str, script: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("files")).expect("files dir");
    std::fs::create_dir_all(dir.join("scripts")).expect("scripts dir");
    std::fs::write(dir.join("scripts/manifest.js"), script).expect("script");
    let config = dir.join("stuntdouble.toml");
    let text = body.replace("{port}", &port.to_string());
    std::fs::write(&config, text).expect("config");
    config
}

/// Insert a `[sandbox]` table into a fixture config.
fn with_sandbox(body: &str, timeout_ms: u64) -> String {
    body.replace(
        "[files]",
        &format!("[sandbox]\nscript_timeout_ms = {timeout_ms}\n\n[files]"),
    )
}

fn reserve_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(BIN).args(args).output().expect("run binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn serve_with_env(config: &Path, env: &[(&str, &str)]) -> Child {
    let mut command = Command::new(BIN);
    command
        .args(["serve", "--config"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().expect("spawn serve")
}

fn wait_ready(port: u16, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(status) = child.try_wait().expect("try_wait") {
            panic!("server exited early with {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("server never accepted connections on port {port}");
}

fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Response {
    request_with_body(port, method, path, headers, "")
}

fn request_with_body(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut raw = String::new();
    let _ = write!(
        raw,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        let _ = write!(raw, "{name}: {value}\r\n");
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    stream.write_all(raw.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read response");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.lines();
    let status_line = lines.next().expect("status line");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .expect("parsable status code");
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    Response {
        status,
        headers,
        body: body.to_string(),
    }
}

fn json_string(body: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = body.find(&marker)? + marker.len();
    let end = start + body[start..].find('"')?;
    Some(body[start..end].to_string())
}

/// One canned response served by the in-test upstream.
struct UpstreamResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Duration,
}

impl UpstreamResponse {
    fn new(status: u16, body: &[u8]) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
            delay: Duration::ZERO,
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// Minimal stdlib HTTP upstream: serves a fixed script of responses, one per
/// accepted connection, so tests exercise the real network path end to end.
struct Upstream {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Upstream {
    fn start(responses: Vec<UpstreamResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream");
        listener
            .set_nonblocking(true)
            .expect("nonblocking upstream");
        let port = listener.local_addr().expect("upstream addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            let mut next = 0;
            while !thread_stop.load(Ordering::Relaxed) && next < responses.len() {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let head = read_upstream_request_head(&mut stream);
                        thread_requests
                            .lock()
                            .expect("upstream requests lock")
                            .push(head);
                        let response = &responses[next];
                        next += 1;
                        let wake = Instant::now() + response.delay;
                        while Instant::now() < wake && !thread_stop.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let reason = match response.status {
                            200 => "OK",
                            302 => "Found",
                            404 => "Not Found",
                            500 => "Internal Server Error",
                            _ => "Status",
                        };
                        let mut raw = format!(
                            "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
                            response.status,
                            response.body.len()
                        );
                        for (name, value) in &response.headers {
                            let _ = write!(raw, "{name}: {value}\r\n");
                        }
                        raw.push_str("\r\n");
                        let _ = stream.write_all(raw.as_bytes());
                        let _ = stream.write_all(&response.body);
                        let _ = stream.flush();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
            requests,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// Request heads received so far, in arrival order.
    fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("upstream requests lock")
            .clone()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_upstream_request_head(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut head = Vec::new();
    let mut chunk = [0_u8; 2048];
    while let Ok(read) = stream.read(&mut chunk) {
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// Add the [upstream] allowlist required by ctx.http.get.
fn with_upstream(body: &str, allow_hosts: &[&str]) -> String {
    with_upstream_timeout(body, allow_hosts, None)
}

/// Add [upstream] with an explicit timeout; `None` keeps the default.
fn with_upstream_timeout(body: &str, allow_hosts: &[&str], timeout_ms: Option<u64>) -> String {
    let hosts = allow_hosts
        .iter()
        .map(|host| format!("\"{host}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let timeout = timeout_ms.map_or(String::new(), |ms| format!("timeout_ms = {ms}\n"));
    body.replace(
        "[files]",
        &format!("[upstream]\nallow_hosts = [{hosts}]\n{timeout}\n[files]"),
    )
}

fn with_server<T>(config_body: &str, run_tests: impl FnOnce(u16) -> T) -> (T, String) {
    with_server_full(config_body, OK_SCRIPT, &[], run_tests)
}

fn with_server_full<T>(
    config_body: &str,
    script: &str,
    env: &[(&str, &str)],
    run_tests: impl FnOnce(u16) -> T,
) -> (T, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = reserve_port();
    let config = fixture_with_script(dir.path(), port, config_body, script);
    serve_and_run(&config, port, env, run_tests)
}

/// Serve one configuration and hand back the test result plus server stderr.
fn serve_and_run<T>(
    config: &Path,
    port: u16,
    env: &[(&str, &str)],
    run_tests: impl FnOnce(u16) -> T,
) -> (T, String) {
    let mut child = serve_with_env(config, env);
    wait_ready(port, &mut child);
    let value = run_tests(port);
    child.kill().expect("kill server");
    let output = child.wait_with_output().expect("collect output");
    (value, String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn validate_accepts_a_good_configuration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fixture(dir.path(), 3000, good_config());
    let (code, stdout, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("valid configuration"), "stdout: {stdout}");
    assert!(stderr.contains("routes: 1"), "stderr: {stderr}");
}

#[test]
fn validate_defaults_server_fields() {
    let minimal = r#"config_version = "1"

[server]

[files]
root = "./files"

[[routes]]
method = "get"
path = "/x"
script = "scripts/manifest.js"
"#;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fixture(dir.path(), 0, minimal);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stderr.contains("127.0.0.1:3000"),
        "defaults missing: {stderr}"
    );
    assert!(stderr.contains("GET /x"), "method not normalised: {stderr}");
}

#[test]
fn validate_reports_missing_route_fields_by_dotted_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = good_config().replace("method = \"GET\"\n", "");
    let config = fixture(dir.path(), 3000, &broken);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "expected configuration error, stderr: {stderr}");
    assert!(
        stderr.contains("routes[0].method: expected string, got missing"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&config.display().to_string()),
        "path missing: {stderr}"
    );
}

#[test]
fn validate_rejects_unknown_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let extra = format!("{}\nbogus = true\n", good_config());
    let config = fixture(dir.path(), 3000, &extra);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("bogus: expected one of"),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_requires_existing_static_file_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let moved = good_config().replace("./files", "./nowhere");
    let config = fixture(dir.path(), 3000, &moved);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("files.root: expected existing directory"),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_rejects_python_scripts_with_the_real_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let python = good_config().replace("scripts/manifest.js", "scripts/manifest.py");
    let config = fixture(dir.path(), 3000, &python);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("Python runtime is not supported"),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_reports_toml_syntax_position() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = good_config().replace("config_version = \"1\"", "config_version = ");
    let config = fixture(dir.path(), 3000, &broken);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("invalid TOML"), "stderr: {stderr}");
    assert!(
        stderr.contains("line") && stderr.contains("column"),
        "position missing: {stderr}"
    );
}

#[test]
fn unmatched_request_gets_404_with_request_id() {
    let (response, _) = with_server(good_config(), |port| request(port, "GET", "/nope", &[]));
    assert_eq!(response.status, 404, "body: {}", response.body);
    assert!(
        response.body.contains("\"error\":\"not_found\""),
        "body: {}",
        response.body
    );
    let in_body = json_string(&response.body, "request_id").expect("request_id in body");
    assert!(!in_body.is_empty(), "empty request_id");
    assert_eq!(
        response.header("x-request-id"),
        Some(in_body.as_str()),
        "header must match body"
    );
}

#[test]
fn matched_route_runs_the_route_script() {
    let (response, _) = with_server(good_config(), |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "ok");
    assert_eq!(
        response.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert!(
        response.header("x-request-id").is_some(),
        "correlation header missing"
    );
}

#[test]
fn method_mismatch_is_reported_as_not_found() {
    let (response, _) = with_server(good_config(), |port| {
        request(port, "POST", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 404, "body: {}", response.body);
}

#[test]
fn client_request_id_is_recorded_but_never_adopted() {
    let (own, stderr) = with_server(good_config(), |port| {
        let response = request(port, "GET", "/nope", &[("X-Request-ID", "caller-supplied")]);
        json_string(&response.body, "request_id")
    });
    let own = own.expect("request_id in body");
    assert_ne!(own, "caller-supplied", "server must mint its own id");
    assert!(
        stderr.contains("caller-supplied"),
        "client id not logged: {stderr}"
    );
}

#[test]
fn each_request_writes_one_structured_log_line() {
    let (statuses, stderr) = with_server(good_config(), |port| {
        vec![
            request(port, "GET", "/demo/documents/manifest/group-a", &[]).status,
            request(port, "GET", "/nope", &[]).status,
        ]
    });
    assert_eq!(statuses, vec![200, 404]);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("\"request_id\""))
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "expected one log line per request: {stderr}"
    );
    assert!(
        lines[0].contains("\"route\":\"manifest\""),
        "route label missing: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"group\":\"group-a\""),
        "params missing: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("\"error\":\"not_found\""),
        "error class missing: {}",
        lines[1]
    );
    assert!(
        lines[0].contains("\"elapsed_ms\":"),
        "duration missing: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"host\":"),
        "listen host missing: {}",
        lines[0]
    );
}

#[test]
fn validate_accepts_sandbox_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fixture(dir.path(), 3000, &with_sandbox(good_config(), 250));
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn validate_rejects_non_positive_sandbox_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fixture(dir.path(), 3000, &with_sandbox(good_config(), 0));
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("sandbox.script_timeout_ms"),
        "stderr: {stderr}"
    );
}

#[test]
fn script_reads_the_ctx_request_snapshot() {
    let script = r#"
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify({
  apiVersion: ctx.apiVersion,
  method: ctx.request.method,
  path: ctx.request.path,
  params: ctx.request.params,
  query: ctx.request.query,
  headers: ctx.request.headers,
  bodyText: ctx.request.bodyText
}));
"#;
    let body = r#"config_version = "1"

[server]
bind = "127.0.0.1"
port = {port}

[files]
root = "./files"

[[routes]]
name = "echo"
method = "POST"
path = "/echo/:name"
script = "scripts/manifest.js"
"#;
    let (response, _) = with_server_full(body, script, &[], |port| {
        request_with_body(
            port,
            "POST",
            "/echo/abc?x=1&y=two",
            &[("X-Trace", "t-1")],
            "payload",
        )
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["apiVersion"], "1");
    assert_eq!(json["method"], "POST");
    assert_eq!(json["path"], "/echo/abc");
    assert_eq!(json["params"]["name"], "abc");
    assert_eq!(json["query"]["x"], "1");
    assert_eq!(json["query"]["y"], "two");
    assert_eq!(json["headers"]["x-trace"], "t-1");
    assert_eq!(json["bodyText"], "payload");
}

#[test]
fn second_respond_call_is_ignored_and_logged() {
    let script = r#"
ctx.respond(201, { "X-First": "yes" }, "first");
ctx.respond(500, {}, "second");
"#;
    let (response, stderr) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body, "first");
    assert_eq!(response.header("x-first"), Some("yes"));
    assert!(stderr.contains("already produced"), "stderr: {stderr}");
}

#[test]
fn script_env_reflects_process_environment() {
    let script = r"ctx.respond(200, {}, ctx.env.METADATA_API_URL);";
    let (response, _) = with_server_full(
        good_config(),
        script,
        &[(
            "METADATA_API_URL",
            "https://metadata.example.com/demo/documents",
        )],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "https://metadata.example.com/demo/documents");
}

#[test]
fn uncaught_script_error_maps_to_500_with_request_id() {
    let script = r#"throw new Error("secret stack detail");"#;
    let (response, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert!(
        response.body.contains("\"error\":\"script_error\""),
        "body: {}",
        response.body
    );
    assert!(
        json_string(&response.body, "request_id").is_some(),
        "body: {}",
        response.body
    );
    assert!(
        !response.body.contains("secret stack detail"),
        "script detail leaked to the client: {}",
        response.body
    );
    assert!(response.header("x-request-id").is_some());
}

#[test]
fn script_without_respond_maps_to_script_no_response() {
    let script = r#"ctx.log.info("nothing produced");"#;
    let (response, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert!(
        response.body.contains("\"error\":\"script_no_response\""),
        "body: {}",
        response.body
    );
    assert!(
        json_string(&response.body, "request_id").is_some(),
        "body: {}",
        response.body
    );
}

#[test]
fn script_timeout_maps_to_500_script_error() {
    let script = "while (true) {}";
    let (response, _) = with_server_full(&with_sandbox(good_config(), 200), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert!(
        response.body.contains("\"error\":\"script_error\""),
        "body: {}",
        response.body
    );
}

#[test]
fn script_logs_reach_server_logs_but_not_the_client() {
    let script = r#"
ctx.log.info("hello from the script");
ctx.log.error("problem", { code: 7 });
ctx.respond(200, {}, "done");
"#;
    let (response, stderr) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "done");
    assert!(stderr.contains("hello from the script"), "stderr: {stderr}");
    assert!(stderr.contains("\"script_logs\""), "stderr: {stderr}");
    assert!(!response.body.contains("hello from the script"));
}

#[test]
fn ctx_http_get_returns_status_headers_text_and_bytes() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, br#"{"hello":"world"}"#)
        .header("Content-Type", "application/json")
        .header("X-Upstream", "yes")]);
    let script = r#"
var r = ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify({
  status: r.status,
  type: r.headers["content-type"],
  upstream: r.headers["x-upstream"],
  text: r.text(),
  bytes: Array.from(r.bytes())
}));
"#;
    let url = upstream.url("/meta");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["status"], 200);
    assert_eq!(json["type"], "application/json");
    assert_eq!(json["upstream"], "yes");
    assert_eq!(json["text"], r#"{"hello":"world"}"#);
    assert_eq!(
        json["bytes"],
        serde_json::json!([
            123, 34, 104, 101, 108, 108, 111, 34, 58, 34, 119, 111, 114, 108, 100, 34, 125
        ])
    );
}

fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn error_class(body: &str) -> Option<String> {
    json_string(body, "error")
}

/// Run a script expected to fail before responding and assert the stable
/// script-error contract used by policy and validation failures.
fn assert_script_error(config_body: &str, script: &str) {
    let (response, _) = with_server_full(config_body, script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn ctx_http_get_treats_4xx_and_5xx_as_data() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(404, b"missing"),
        UpstreamResponse::new(500, b"boom"),
    ]);
    let script = r#"
var missing = ctx.http.get(ctx.env.UPSTREAM_URL + "/missing");
var broken = ctx.http.get(ctx.env.UPSTREAM_URL + "/broken");
ctx.respond(299, {}, String(missing.status) + ":" + missing.text() + "|" + String(broken.status) + ":" + broken.text());
"#;
    let url = upstream.url("");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 299, "body: {}", response.body);
    assert_eq!(response.body, "404:missing|500:boom");
}

#[test]
fn ctx_http_get_rejects_non_http_protocol_as_script_error() {
    let script = r#"
ctx.http.get("file:///etc/passwd");
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(good_config(), script);
}

#[test]
fn ctx_http_get_denies_hosts_without_an_upstream_allowlist() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/");
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(good_config(), script);
}

#[test]
fn ctx_http_get_rejects_host_not_in_allowlist_as_script_error() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/");
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["allowed.example"]), script);
}

#[test]
fn ctx_http_get_rejects_unknown_opts_key_as_script_error() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/", { retries: 1 });
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_rejects_non_positive_timeout_as_script_error() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/", { timeout_ms: 0 });
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_rejects_null_timeout_as_script_error() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/", { timeout_ms: null });
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_rejects_non_finite_timeout_as_script_error() {
    let script = r#"
ctx.http.get("http://127.0.0.1:1/", { timeout_ms: Infinity });
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_rejects_symbol_opts_key_as_script_error() {
    let script = r#"
var opts = {};
opts[Symbol("retries")] = 1;
ctx.http.get("http://127.0.0.1:1/", opts);
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_rejects_inherited_opts_keys_as_script_error() {
    let script = r#"
var opts = Object.create({ retries: 1 });
ctx.http.get("http://127.0.0.1:1/", opts);
ctx.respond(200, {}, "should not respond");
"#;
    assert_script_error(&with_upstream(good_config(), &["127.0.0.1"]), script);
}

#[test]
fn ctx_http_get_script_thrown_upstream_code_is_not_mapped_to_502() {
    assert_script_error(
        good_config(),
        r#"
throw { code: "upstream_unreachable" };
"#,
    );
}

#[test]
fn ctx_http_get_bytes_preserve_binary_values() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, &[0x00, 0xFF, 0x10, 0x80])]);
    let script = r"
var bytes = ctx.http.get(ctx.env.UPSTREAM_URL).bytes();
ctx.respond(200, {}, JSON.stringify(Array.from(bytes)));
";
    let url = upstream.url("/binary");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "[0,255,16,128]");
}

#[test]
fn ctx_http_get_rejects_oversized_body_as_script_error() {
    let body = vec![b'x'; 8 * 1024 * 1024 + 1];
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, &body)]);
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let url = upstream.url("/large");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn ctx_http_get_ignores_proxy_environment() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"direct")]);
    let script = r"
var r = ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, r.text());
";
    let url = upstream.url("/meta");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[
            ("UPSTREAM_URL", url.as_str()),
            ("HTTP_PROXY", "http://127.0.0.1:1"),
            ("HTTPS_PROXY", "http://127.0.0.1:1"),
            ("ALL_PROXY", "http://127.0.0.1:1"),
        ],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "direct");
}

#[test]
fn ctx_http_get_follows_up_to_three_allowlisted_redirects() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(302, b"").header("Location", "/one"),
        UpstreamResponse::new(302, b"").header("Location", "/two"),
        UpstreamResponse::new(302, b"").header("Location", "/three"),
        UpstreamResponse::new(200, b"done"),
    ]);
    let script = r"
var r = ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, r.text());
";
    let url = upstream.url("/start");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "done");
}

#[test]
fn ctx_http_get_rejects_redirect_chain_longer_than_three_hops() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(302, b"").header("Location", "/one"),
        UpstreamResponse::new(302, b"").header("Location", "/two"),
        UpstreamResponse::new(302, b"").header("Location", "/three"),
        UpstreamResponse::new(302, b"").header("Location", "/four"),
        UpstreamResponse::new(200, b"should not be reached"),
    ]);
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let url = upstream.url("/start");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn ctx_http_get_rejects_redirect_to_non_allowlisted_host() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(302, b"").header("Location", "http://localhost:1/nope")
    ]);
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let url = upstream.url("/start");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn ctx_http_get_transport_failure_is_catchable() {
    let port = closed_port();
    let url = format!("http://127.0.0.1:{port}/meta");
    let script = r#"
try {
  ctx.http.get(ctx.env.UPSTREAM_URL);
  ctx.respond(500, {}, "expected a transport failure");
} catch (error) {
  ctx.respond(200, {}, error.code);
}
"#;
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "upstream_unreachable");
}

#[test]
fn uncaught_ctx_http_get_transport_failure_maps_to_502() {
    let port = closed_port();
    let url = format!("http://127.0.0.1:{port}/meta");
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("upstream_unreachable")
    );
    assert!(json_string(&response.body, "request_id").is_some());
    assert!(response.header("x-request-id").is_some());
}

#[test]
fn ctx_http_get_per_call_timeout_maps_to_502() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(200, b"late").delay(Duration::from_secs(30))
    ]);
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL, { timeout_ms: 100 });
ctx.respond(200, {}, "should not respond");
"#;
    let url = upstream.url("/slow");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("upstream_unreachable")
    );
}

#[test]
fn ctx_http_get_uses_upstream_timeout_config() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(200, b"late").delay(Duration::from_secs(30))
    ]);
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let url = upstream.url("/slow");
    let (response, _) = with_server_full(
        &with_upstream_timeout(good_config(), &["127.0.0.1"], Some(100)),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("upstream_unreachable")
    );
}

#[test]
fn validate_accepts_upstream_configuration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = fixture(
        dir.path(),
        3000,
        &with_upstream_timeout(good_config(), &["metadata.example.com"], Some(500)),
    );
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn validate_rejects_unknown_upstream_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let body = good_config().replace(
        "[files]",
        "[upstream]\nallow_hosts = [\"example.com\"]\nbogus = true\n\n[files]",
    );
    let config = fixture(dir.path(), 3000, &body);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("upstream.bogus: expected one of"),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_rejects_invalid_allow_hosts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let body = good_config().replace("[files]", "[upstream]\nallow_hosts = [\"\", 7]\n\n[files]");
    let config = fixture(dir.path(), 3000, &body);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("upstream.allow_hosts[0]"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("upstream.allow_hosts[1]"),
        "stderr: {stderr}"
    );
}

// Demo fixture: the in-repo configuration plus script from
// plans/demo-document-catalog.md §3.1 must produce the catalog contract
// through the built binary and a stdlib fake upstream.

/// Root of the in-repo demo fixture.
const DEMO_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/demo");

/// Copy the repository demo fixture into `dir`, moving it onto the test port
/// and allowlisting the loopback fake upstream. Returns the config path.
fn demo_fixture(dir: &Path, port: u16) -> PathBuf {
    let source = Path::new(DEMO_DIR);
    let config =
        std::fs::read_to_string(source.join("stuntdouble.toml")).expect("read demo config");
    let port_marker = "port = 3000";
    let hosts_marker = "allow_hosts = [\"metadata.example.com\"]";
    assert!(config.contains(port_marker), "demo config moved: {config}");
    assert!(config.contains(hosts_marker), "demo config moved: {config}");
    let config = config
        .replacen(port_marker, &format!("port = {port}"), 1)
        .replacen(
            hosts_marker,
            "allow_hosts = [\"metadata.example.com\", \"127.0.0.1\"]",
            1,
        );
    std::fs::create_dir_all(dir.join("files")).expect("files dir");
    std::fs::create_dir_all(dir.join("scripts")).expect("scripts dir");
    std::fs::copy(
        source.join("scripts/manifest.js"),
        dir.join("scripts/manifest.js"),
    )
    .expect("copy demo script");
    let path = dir.join("stuntdouble.toml");
    std::fs::write(&path, config).expect("write demo config");
    path
}

/// Serve a copy of the repository demo fixture with `env` set.
fn with_demo<T>(env: &[(&str, &str)], run_tests: impl FnOnce(u16) -> T) -> (T, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = reserve_port();
    let config = demo_fixture(dir.path(), port);
    serve_and_run(&config, port, env, run_tests)
}

fn demo_manifest_request(port: u16, headers: &[(&str, &str)]) -> Response {
    request(port, "GET", "/demo/documents/manifest/group-a", headers)
}

/// Run one manifest request against a served demo fixture whose
/// `METADATA_API_URL` points at `upstream`.
fn manifest_response(upstream: &Upstream, headers: &[(&str, &str)]) -> Response {
    let url = upstream.url("/demo/documents");
    let (response, _) = with_demo(&[("METADATA_API_URL", url.as_str())], |port| {
        demo_manifest_request(port, headers)
    });
    response
}

fn assert_metadata_bad_gateway(response: &Response) {
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("metadata_bad_gateway"),
        "body: {}",
        response.body
    );
}

#[test]
fn repository_demo_configuration_validates() {
    let config = format!("{DEMO_DIR}/stuntdouble.toml");
    let (code, _, stderr) = run(&["validate", "--config", &config]);
    assert_eq!(code, 0, "stderr: {stderr}");
}

#[test]
fn demo_manifest_route_returns_the_catalog_csv() {
    let metadata = r#"{"data":[
{"code":"DOC-0001","title":"示例设备 A 安装手册","system_code":"SYS-A","pdf_url":"http://files.example.com/demo/documents/DOC-0001.pdf"},
{"code":"DOC-0002","title":"示例设备 B 运行手册","system_code":"SYS-B","pdf_url":"http://files.example.com/demo/documents/DOC-0002.pdf"}
]}"#;
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, metadata.as_bytes())]);
    let response = manifest_response(&upstream, &[]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        response.body,
        "文件编码,文件标题,系统代码\n\
         DOC-0001,示例设备 A 安装手册,SYS-A\n\
         DOC-0002,示例设备 B 运行手册,SYS-B\n"
    );
}

#[test]
fn demo_manifest_route_escapes_csv_fields() {
    let metadata = r#"{"data":[
{"code":"DOC-0003","title":"示例,设备 \"A\"\n第二行","system_code":"SYS,C"},
{"code":"DOC-0004","title":"回车\r换行","system_code":"SYS-D"}
]}"#;
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, metadata.as_bytes())]);
    let response = manifest_response(&upstream, &[]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        "文件编码,文件标题,系统代码\n\
         DOC-0003,\"示例,设备 \"\"A\"\"\n第二行\",\"SYS,C\"\n\
         DOC-0004,\"回车\r换行\",SYS-D\n"
    );
}

#[test]
fn demo_manifest_route_returns_header_only_for_empty_data() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"{\"data\":[]}")]);
    let response = manifest_response(&upstream, &[]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "文件编码,文件标题,系统代码\n");
}

#[test]
fn demo_manifest_route_maps_metadata_non_2xx_to_502() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(404, b"missing"),
        UpstreamResponse::new(503, b"unavailable"),
    ]);
    assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
    assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
}

#[test]
fn demo_manifest_route_maps_unusable_metadata_to_502() {
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(200, b"not-json"),
        UpstreamResponse::new(200, b"{}"),
        UpstreamResponse::new(200, b"{\"data\":{}}"),
    ]);
    assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
    assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
    assert_metadata_bad_gateway(&manifest_response(&upstream, &[]));
}

#[test]
fn demo_manifest_route_maps_unreachable_metadata_to_502() {
    let url = format!("http://127.0.0.1:{}/demo/documents", closed_port());
    let (response, _) = with_demo(&[("METADATA_API_URL", url.as_str())], |port| {
        demo_manifest_request(port, &[])
    });
    assert_metadata_bad_gateway(&response);
}

#[test]
fn demo_manifest_route_does_not_forward_client_request_id() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"{\"data\":[]}")]);
    let response = manifest_response(&upstream, &[("X-Request-ID", "key")]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert!(
        response.header("x-request-id").is_some(),
        "server must still mint its own request id"
    );
    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "upstream calls: {requests:?}");
    let head = requests[0].to_ascii_lowercase();
    // Positive control: the capture holds a real request head, so the
    // negative assertions below cannot pass on an empty recording.
    assert!(head.contains("host:"), "recorded head: {}", requests[0]);
    assert!(
        head.starts_with("get /demo/documents "),
        "unexpected upstream request: {}",
        requests[0]
    );
    assert!(
        !head.contains("x-request-id"),
        "client header leaked upstream: {}",
        requests[0]
    );
}
