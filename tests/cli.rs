//! End-to-end checks through the only seam: the built binary plus real HTTP.
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// Fixture ports are allocated explicitly instead of asking the kernel for an
/// ephemeral one: `bind(127.0.0.1:0)` draws from the same range as client
/// sockets, so a concurrent test could take a port another test just proved
/// closed and then answer that test's "unreachable upstream" call, or bind a
/// port that is meant to stay free. Walking a private range keeps fixture ports
/// disjoint; it starts below the common ephemeral ranges so client sockets do
/// not land in it either.
const FIXTURE_PORT_START: u16 = 20_000;
const FIXTURE_PORT_END: u16 = 30_000;
const FIXTURE_PORT_SPAN: u16 = FIXTURE_PORT_END - FIXTURE_PORT_START + 1;

/// Bind the next free fixture port. Callers that must own the port keep the
/// listener (`Upstream`); callers that only need the number drop it.
fn bind_fixture_port() -> TcpListener {
    static NEXT: OnceLock<Mutex<u16>> = OnceLock::new();
    let mut next = NEXT
        .get_or_init(|| Mutex::new(fixture_port_start()))
        .lock()
        .expect("fixture port cursor");
    for _ in 0..FIXTURE_PORT_SPAN {
        let port = *next;
        *next = if port >= FIXTURE_PORT_END {
            FIXTURE_PORT_START
        } else {
            port + 1
        };
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return listener;
        }
    }
    panic!("no free fixture port in {FIXTURE_PORT_START}..={FIXTURE_PORT_END}");
}

/// Concurrent copies of this suite must not walk the same ports in lockstep,
/// so the cursor starts at a process-specific offset inside the private range.
fn fixture_port_start() -> u16 {
    let offset = std::process::id().wrapping_mul(4099) % u32::from(FIXTURE_PORT_SPAN);
    FIXTURE_PORT_START + u16::try_from(offset).expect("offset inside fixture range")
}

/// Port handed to a served child, which binds it after this returns.
fn next_fixture_port() -> u16 {
    bind_fixture_port()
        .local_addr()
        .expect("fixture addr")
        .port()
}

/// Port that has to stay free for the whole test: the cursor never hands it out
/// again, so no fixture in this process can bind it.
fn closed_port() -> u16 {
    next_fixture_port()
}

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(BIN).args(args).output().expect("run binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Fresh ports a served fixture tries before the startup failure is fatal.
const SERVE_ATTEMPTS: usize = 5;

/// Outcome of waiting for a served child to announce its listening socket.
enum Started {
    Ready,
    /// The child exited before owning the port: usually a concurrent test took
    /// it, but a genuine startup failure looks the same here. The message
    /// carries the child's stderr.
    Exited(String),
    /// The child stayed alive past the deadline without announcing a listener.
    Silent(String),
}

/// Start `serve` for `prepare(port)`'s config with `env`/`args`, returning the
/// live child, its stderr log and the port it owns.
///
/// Readiness cannot be a bare connect probe: `next_fixture_port()` releases the
/// port before the child binds it, so anything else on the machine can take it
/// in between, answer the probe and leave the child dying with "Address already
/// in use" while the client talks to the wrong process. Only the child's own
/// announcement proves readiness; a lost port is retried on a fresh one.
/// `prepare` runs per attempt so the fixture lands on the port the child bound.
fn start_serve(
    prepare: impl Fn(u16) -> PathBuf,
    env: &[(&str, &str)],
    args: &[&str],
) -> (Child, PathBuf, u16) {
    let mut last_failure = String::new();
    for _ in 1..=SERVE_ATTEMPTS {
        let port = next_fixture_port();
        let config = prepare(port);
        let log = config.with_extension("serve.stderr");
        let mut child = serve_with_args(&config, env, args, &log);
        match wait_ready(port, &mut child, &log) {
            Started::Ready => return (child, log, port),
            Started::Exited(message) => {
                stop(&mut child);
                last_failure = message;
            }
            Started::Silent(message) => {
                stop(&mut child);
                panic!("{message}");
            }
        }
    }
    panic!("serve did not own a fresh port in {SERVE_ATTEMPTS} attempts: {last_failure}");
}

/// Serve `prepare(port)`'s config and run `run_tests` against it once the child
/// owns the announced port.
fn serve_and_run<T>(
    prepare: impl Fn(u16) -> PathBuf,
    env: &[(&str, &str)],
    args: &[&str],
    run_tests: impl FnOnce(u16) -> T,
) -> (T, String) {
    let (mut child, log, port) = start_serve(prepare, env, args);
    let value = run_tests(port);
    wait_for_request_log(&log);
    stop(&mut child);
    (value, std::fs::read_to_string(&log).unwrap_or_default())
}

/// Streamed responses write their request log from a spawned relay task after
/// the client already holds the body, and multi-request fixtures finish with
/// more than one line. Wait until the request-log line count stops growing
/// before stopping the process; assertions still fail if a line never lands.
fn wait_for_request_log(log: &Path) {
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut last_count = 0;
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let count = text
            .lines()
            .filter(|line| line.contains("\"request_id\""))
            .count();
        if count > 0 && count == last_count {
            return;
        }
        last_count = count;
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    child.wait().expect("reap server");
}

/// Spawn `serve` with its stderr captured in `log`, which carries both the
/// startup announcement and the request logs the tests assert on.
fn serve_with_args(config: &Path, env: &[(&str, &str)], args: &[&str], log: &Path) -> Child {
    let mut command = Command::new(BIN);
    command
        .args(["serve", "--config"])
        .arg(config)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(log).expect("serve log")));
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().expect("spawn serve")
}

fn wait_ready(port: u16, child: &mut Child, log: &Path) -> Started {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if text.lines().any(|line| listener_announcement(line, port)) {
            return Started::Ready;
        }
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Started::Exited(format!("server exited early with {status}: {text}"));
        }
        if Instant::now() >= deadline {
            return Started::Silent(format!("server never announced port {port}: {text}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `serve` prints this once its socket is bound, e.g.
/// `stuntdouble listening on http://127.0.0.1:34567`.
fn listener_announcement(line: &str, port: u16) -> bool {
    line.strip_prefix("stuntdouble listening on http://")
        .is_some_and(|addr| addr.ends_with(&format!(":{port}")))
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

/// Send one request and drop the connection after reading the response head
/// plus one body byte, so the server observes a client that left mid-body.
fn abort_after_response_head(port: u16) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let raw = "GET /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    stream.write_all(raw.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).expect("read response");
        assert_ne!(read, 0, "server closed before the client could abort");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(head_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            if bytes.len() > head_end + 4 {
                return;
            }
        }
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
    content_length: Option<usize>,
}

impl UpstreamResponse {
    fn new(status: u16, body: &[u8]) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
            delay: Duration::ZERO,
            content_length: None,
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

    /// Announce a length different from the bytes actually written, so tests
    /// can exercise a truncated upstream body.
    fn content_length(mut self, length: usize) -> Self {
        self.content_length = Some(length);
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
        Self::start_with(|_| responses)
    }

    /// Variant for tests whose canned payloads must point back at the fake
    /// upstream: the responses are built once the bound port is known.
    fn start_with(build: impl FnOnce(u16) -> Vec<UpstreamResponse>) -> Self {
        let listener = bind_fixture_port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking upstream");
        let port = listener.local_addr().expect("upstream addr").port();
        let responses = build(port);
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
                            201 => "Created",
                            206 => "Partial Content",
                            207 => "Multi-Status",
                            302 => "Found",
                            404 => "Not Found",
                            500 => "Internal Server Error",
                            503 => "Service Unavailable",
                            _ => "Status",
                        };
                        let content_length = response.content_length.unwrap_or(response.body.len());
                        let mut raw = format!(
                            "HTTP/1.1 {} {reason}\r\nContent-Length: {content_length}\r\nConnection: close\r\n",
                            response.status,
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
    serve_and_run(
        |port| fixture_with_script(dir.path(), port, config_body, script),
        env,
        &[],
        run_tests,
    )
}

/// A port handed to a served child is probed and released before the child
/// binds it, so another listener can take it in between. Readiness must come
/// from our own child, never from the thief's listener, and the fixture must be
/// retried onto a port it owns.
#[test]
fn stolen_probed_port_is_retried_until_the_served_child_owns_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let squatted: RefCell<Vec<TcpListener>> = RefCell::new(Vec::new());
    let first_attempt = RefCell::new(true);
    let (response, stderr) = serve_and_run(
        |port| {
            if first_attempt.replace(false) {
                // Deterministic stand-in for the concurrent test that wins the
                // race. If something already took the port, the child loses it
                // the same way, so the squat is best effort either way.
                if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
                    squatted.borrow_mut().push(listener);
                }
            }
            fixture_with_script(dir.path(), port, good_config(), OK_SCRIPT)
        },
        &[],
        &[],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(
        response.status, 200,
        "body: {} stderr: {stderr}",
        response.body
    );
    assert_eq!(response.body, "ok", "stderr: {stderr}");
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
fn validate_rejects_unknown_config_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let version_two = good_config().replace("config_version = \"1\"", "config_version = \"2\"");
    let config = fixture(dir.path(), 3000, &version_two);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("config_version: expected \"1\", got string \"2\""),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_rejects_non_ip_server_bind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hostname = good_config().replace("bind = \"127.0.0.1\"", "bind = \"localhost\"");
    let config = fixture(dir.path(), 3000, &hostname);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("server.bind: expected IP address literal, got string \"localhost\""),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_rejects_empty_routes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = r#"config_version = "1"
routes = []

[server]

[files]
root = "./files"
"#;
    let config = fixture(dir.path(), 3000, empty);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("routes: expected at least one route table, got array of 0"),
        "stderr: {stderr}"
    );
}

#[test]
fn validate_reports_route_and_files_root_violations_together() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = r#"config_version = "1"
routes = []

[server]

[files]
root = "./nowhere"
"#;
    let config = fixture(dir.path(), 3000, broken);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("routes: expected at least one route table"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("files.root: expected existing directory"),
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
fn verbose_adds_a_stable_detail_to_script_errors() {
    let script = r#"throw new Error("secret stack detail");"#;
    let (plain, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(plain.status, 500, "body: {}", plain.body);
    assert!(
        json_string(&plain.body, "detail").is_none(),
        "detail appeared without --verbose: {}",
        plain.body
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let (verbose, _) = serve_and_run(
        |port| fixture_with_script(dir.path(), port, good_config(), script),
        &[],
        &["--verbose"],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(verbose.status, 500, "body: {}", verbose.body);
    assert_eq!(
        json_string(&verbose.body, "detail").as_deref(),
        Some("script execution failed")
    );
    assert!(
        !verbose.body.contains("secret stack detail"),
        "script message leaked with --verbose: {}",
        verbose.body
    );
}

#[test]
fn verbose_upstream_detail_omits_internal_addresses() {
    let upstream_port = closed_port();
    let url = format!("http://127.0.0.1:{upstream_port}/private/token");
    let script = r#"
ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, "should not respond");
"#;
    let dir = tempfile::tempdir().expect("tempdir");
    let config_body = with_upstream(good_config(), &["127.0.0.1"]);
    let (response, _) = serve_and_run(
        |port| fixture_with_script(dir.path(), port, &config_body, script),
        &[("UPSTREAM_URL", url.as_str())],
        &["--verbose"],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        json_string(&response.body, "detail").as_deref(),
        Some("upstream transport failure: transport")
    );
    assert!(
        !response.body.contains("127.0.0.1"),
        "upstream host leaked: {}",
        response.body
    );
    assert!(
        !response
            .body
            .contains(&format!("127.0.0.1:{upstream_port}")),
        "upstream address leaked: {}",
        response.body
    );
    assert!(
        !response.body.contains("/private/token"),
        "upstream path leaked: {}",
        response.body
    );
}

#[test]
fn error_messages_do_not_carry_request_bodies_into_logs() {
    let script = r"throw new Error(ctx.request.bodyText);";
    let request_body = "request-body-secret";
    let (response, stderr) = with_server_full(good_config(), script, &[], |port| {
        request_with_body(
            port,
            "GET",
            "/demo/documents/manifest/group-a",
            &[],
            request_body,
        )
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert!(
        !stderr.contains(request_body),
        "request body leaked into logs: {stderr}"
    );
    assert!(
        !response.body.contains(request_body),
        "request body leaked into the response: {}",
        response.body
    );
}

#[test]
fn structured_logs_record_upstream_chain_sizes_and_allowlisted_headers() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"upstream-secret-body")
        .header("Content-Type", "text/plain")
        .header("X-Upstream-Secret", "upstream-secret-header")]);
    let script = r#"
var r = ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, { "Content-Type": "text/plain", "X-Response-Secret": "response-secret" }, "client-body");
"#;
    let url = upstream.url("/meta?token=query-secret");
    let request_body = "request-body";
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| {
            request_with_body(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[
                    ("Content-Type", "text/plain"),
                    ("Range", "bytes=0-4"),
                    ("Authorization", "Bearer request-secret"),
                ],
                request_body,
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["route"], "manifest");
    assert_eq!(log["status"], 200);
    assert_eq!(
        log["request_body_bytes"].as_u64(),
        Some(request_body.len() as u64)
    );
    assert_eq!(
        log["response_body_bytes"].as_u64(),
        Some(response.body.len() as u64)
    );
    assert!(log["script_duration_ms"].is_number(), "log: {line}");
    let call = &log["upstream_calls"][0];
    assert_eq!(call["api"], "http.get");
    assert_eq!(call["host"], "127.0.0.1");
    assert_eq!(call["path"], "/meta");
    assert_eq!(call["status"], 200);
    assert_eq!(
        call["response_bytes"],
        u64::try_from("upstream-secret-body".len()).expect("body length")
    );
    assert_eq!(call["redirects"], 0);
    assert!(call["duration_ms"].is_number(), "log: {line}");
    assert!(call["error"].is_null(), "log: {line}");
    assert_eq!(log["request_headers"]["content-type"], "text/plain");
    assert_eq!(log["request_headers"]["range"], "bytes=0-4");
    assert!(
        log["request_headers"].get("authorization").is_none(),
        "sensitive request header logged: {line}"
    );
    assert_eq!(log["response_headers"]["content-type"], "text/plain");
    assert!(
        log["response_headers"].get("x-response-secret").is_none(),
        "sensitive response header logged: {line}"
    );
    for secret in [
        "request-body",
        "client-body",
        "upstream-secret-body",
        "upstream-secret-header",
        "response-secret",
        "query-secret",
        "request-secret",
    ] {
        assert!(!line.contains(secret), "secret {secret:?} logged: {line}");
    }
}

#[test]
fn client_request_id_reaches_logs_but_not_upstream() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"ok")]);
    let script = r"
var r = ctx.http.get(ctx.env.UPSTREAM_URL);
ctx.respond(200, {}, r.text());
";
    let url = upstream.url("/meta");
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| {
            request(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[("X-Request-ID", "caller-supplied")],
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    let heads = upstream.requests();
    assert_eq!(heads.len(), 1, "expected one upstream call");
    assert!(
        !heads[0].to_ascii_lowercase().contains("x-request-id"),
        "client request id was forwarded upstream: {}",
        heads[0]
    );
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["client_request_id"], "caller-supplied");
    assert_ne!(log["request_id"], "caller-supplied");
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
    assert!(
        lines[0].contains("\"script_duration_ms\":"),
        "script duration missing: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"request_body_bytes\":"),
        "request size missing: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"response_body_bytes\":"),
        "response size missing: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"upstream_calls\":"),
        "upstream chain missing: {}",
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
fn ctx_respond_rejects_invalid_header_name_as_script_error() {
    let script = r#"ctx.respond(200, { "Bad Header": "x" }, "no");"#;
    let (response, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("script_error"),
        "body: {}",
        response.body
    );
}

#[test]
fn ctx_respond_rejects_invalid_header_value_as_script_error() {
    let script = r#"
try {
  ctx.respond(200, { "X-Test": "bad\nvalue" }, "no");
  ctx.respond(500, {}, "not caught");
} catch (error) {
  ctx.respond(502, {}, error.code || "missing");
}
"#;
    let (response, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(response.body, "script_error", "body: {}", response.body);
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
    let ((elapsed, response), _) =
        with_server_full(&with_sandbox(good_config(), 200), script, &[], |port| {
            let started = Instant::now();
            let response = request(port, "GET", "/demo/documents/manifest/group-a", &[]);
            (started.elapsed(), response)
        });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert!(
        response.body.contains("\"error\":\"script_error\""),
        "body: {}",
        response.body
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "configured timeout was not enforced: {elapsed:?}"
    );
}

#[test]
fn scripts_cannot_reach_raw_host_capabilities() {
    let script = r#"
var names = ["fetch", "fs", "process", "require", "socket", "__sd_http_get", "__sd_http_pipe", "__sd_validate_headers"];
var leaked = [];
for (var i = 0; i < names.length; i++) {
  if (typeof globalThis[names[i]] !== "undefined") { leaked.push(names[i]); }
}
ctx.respond(200, {}, leaked.length === 0 ? "clean" : leaked.join(","));
"#;
    let (response, _) = with_server_full(good_config(), script, &[], |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "clean", "raw host globals leaked");
}

#[test]
fn failing_route_leaves_other_routes_and_the_server_healthy() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("scripts dir");
    std::fs::write(
        dir.path().join("scripts/runaway.js"),
        "function dive() { dive(); }\ndive();\n",
    )
    .expect("runaway script");
    std::fs::write(dir.path().join("scripts/ok.js"), OK_SCRIPT).expect("ok script");
    let config = dir.path().join("stuntdouble.toml");
    let (responses, stderr) = serve_and_run(
        |port| {
            std::fs::write(
                &config,
                format!(
                    r#"config_version = "1"

[server]
bind = "127.0.0.1"
port = {port}

[files]
root = "./files"

[[routes]]
name = "runaway"
method = "GET"
path = "/runaway"
script = "scripts/runaway.js"

[[routes]]
name = "healthy"
method = "GET"
path = "/healthy"
script = "scripts/ok.js"
"#
                ),
            )
            .expect("config");
            config.clone()
        },
        &[],
        &[],
        |port| {
            (
                request(port, "GET", "/runaway", &[]),
                request(port, "GET", "/healthy", &[]),
                request(port, "GET", "/runaway", &[]),
            )
        },
    );
    let (first, healthy, second) = responses;
    assert_eq!(first.status, 500, "body: {} stderr: {stderr}", first.body);
    assert_eq!(error_class(&first.body).as_deref(), Some("script_error"));
    assert_eq!(healthy.status, 200, "stderr: {stderr}");
    assert_eq!(healthy.body, "ok");
    assert_eq!(second.status, 500, "body: {} stderr: {stderr}", second.body);
    assert_eq!(error_class(&second.body).as_deref(), Some("script_error"));
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
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "done");
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["upstream_calls"][0]["redirects"], 3);
    assert_eq!(log["upstream_calls"][0]["status"], 200);
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
    let (response, stderr) = with_server_full(
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
    assert!(json_string(&response.body, "detail").is_none());
    assert!(response.header("x-request-id").is_some());
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["upstream_calls"][0]["error"], "upstream_unreachable");
    assert_eq!(log["upstream_calls"][0]["kind"], "transport");
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

// `ctx.http.pipe` contract: the upstream body streams to the client around
// the script heap, the script owns status and headers, and policy rejections
// never masquerade as upstream failures.

#[test]
fn ctx_http_pipe_streams_upstream_bytes_with_status_and_headers() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"%PDF-streamed")]);
    let script = r#"
var produced = ctx.http.pipe(ctx.env.UPSTREAM_URL, {
  status: 201,
  headers: { "Content-Type": "application/pdf", "X-Piped": "yes" }
});
if (!produced) { ctx.respond(500, {}, "pipe did not produce a response"); }
"#;
    let url = upstream.url("/file.pdf");
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.header("content-type"), Some("application/pdf"));
    assert_eq!(response.header("x-piped"), Some("yes"));
    assert_eq!(response.body, "%PDF-streamed");
    assert_eq!(
        response.header("content-length"),
        Some("13"),
        "upstream length must travel with the stream"
    );
    assert_eq!(upstream.requests().len(), 1);
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["error"], "");
    assert_eq!(log["response_body_bytes"], 13);
    let call = &log["upstream_calls"][0];
    assert_eq!(call["api"], "http.pipe");
    assert_eq!(call["status"], 200);
    assert_eq!(call["response_bytes"], 13);
    assert!(call["duration_ms"].is_number(), "log: {line}");
    assert!(call["error"].is_null(), "log: {line}");
}

#[test]
fn ctx_http_pipe_mid_stream_failure_is_logged_after_headers() {
    let body = b"short-body";
    let upstream = Upstream::start(vec![
        UpstreamResponse::new(200, body).content_length(body.len() + 54)
    ]);
    let script = r#"
var produced = ctx.http.pipe(ctx.env.UPSTREAM_URL);
if (!produced) { ctx.respond(500, {}, "pipe did not produce a response"); }
"#;
    let url = upstream.url("/file.pdf");
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    // hyper may drop a buffered chunk when the body stream errors, so the
    // client can see fewer bytes than the relay counted.
    assert!(
        response.body.len() <= body.len(),
        "client body exceeded the upstream body: {}",
        response.body
    );
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["status"], 200);
    assert_eq!(log["error"], "upstream_stream_error");
    assert_eq!(
        log["response_body_bytes"],
        u64::try_from(body.len()).expect("body length")
    );
    let call = &log["upstream_calls"][0];
    assert_eq!(call["api"], "http.pipe");
    assert_eq!(call["status"], 200);
    assert_eq!(
        call["response_bytes"],
        u64::try_from(body.len()).expect("body length")
    );
    assert_eq!(call["error"], "upstream_stream_error");
    assert_eq!(call["kind"], "transport");
    assert!(call["duration_ms"].is_number(), "log: {line}");
}

#[test]
fn ctx_http_pipe_client_disconnect_mid_body_is_logged() {
    // Large enough that the relay cannot finish before the client aborts: the
    // harness reads one body byte and closes the socket.
    let body = vec![b'x'; 8 * 1024 * 1024];
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, &body)]);
    let script = r#"
var produced = ctx.http.pipe(ctx.env.UPSTREAM_URL);
if (!produced) { ctx.respond(500, {}, "pipe did not produce a response"); }
"#;
    let url = upstream.url("/file.pdf");
    let ((), stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        abort_after_response_head,
    );
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["status"], 200);
    assert_eq!(log["error"], "client_disconnected", "log: {line}");
    let call = &log["upstream_calls"][0];
    assert_eq!(call["api"], "http.pipe");
    assert_eq!(call["status"], 200);
    let relayed = call["response_bytes"].as_u64().expect("relayed byte count");
    assert!(
        relayed < u64::try_from(body.len()).expect("body length"),
        "the relay finished before the client aborted: {line}"
    );
}

#[test]
fn ctx_http_pipe_rejects_invalid_opts_and_headers_as_script_error() {
    let scripts = [
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { retries: 1 }); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { status: 0 }); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { status: 600 }); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { status: "200" }); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, []); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, null); ctx.respond(200, {}, "no");"#,
        r#"var opts = Object.create({ status: 200 }); ctx.http.pipe(ctx.env.UPSTREAM_URL, opts); ctx.respond(200, {}, "no");"#,
        r#"var opts = {}; opts[Symbol("extra")] = 1; ctx.http.pipe(ctx.env.UPSTREAM_URL, opts); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { headers: { "Bad Header": "x" } }); ctx.respond(200, {}, "no");"#,
        r#"ctx.http.pipe(ctx.env.UPSTREAM_URL, { headers: { "X-Test": "bad\nvalue" } }); ctx.respond(200, {}, "no");"#,
    ];
    for script in scripts {
        let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"%PDF")]);
        let url = upstream.url("/file.pdf");
        let (response, _) = with_server_full(
            &with_upstream(good_config(), &["127.0.0.1"]),
            script,
            &[("UPSTREAM_URL", url.as_str())],
            |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
        );
        assert_eq!(
            response.status, 500,
            "script: {script} body: {}",
            response.body
        );
        assert_eq!(
            error_class(&response.body).as_deref(),
            Some("script_error"),
            "script: {script} body: {}",
            response.body
        );
        assert!(
            upstream.requests().is_empty(),
            "invalid opts reached the upstream: {script}"
        );
    }
}

#[test]
fn ctx_http_pipe_invalid_headers_are_catchable_before_upstream() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"%PDF")]);
    let script = r#"
try {
  ctx.http.pipe(ctx.env.UPSTREAM_URL, { headers: { "Bad Header": "x" } });
  ctx.respond(500, {}, "not caught");
} catch (error) {
  ctx.respond(502, {}, error.code || "missing");
}
"#;
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(response.body, "script_error", "body: {}", response.body);
    assert!(
        upstream.requests().is_empty(),
        "invalid headers reached the upstream"
    );
}

#[test]
fn ctx_http_pipe_uncaught_non_2xx_is_a_script_error() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(404, b"missing")]);
    let script = r#"ctx.http.pipe(ctx.env.UPSTREAM_URL); ctx.respond(200, {}, "no");"#;
    let url = upstream.url("/file.pdf");
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
fn ctx_http_pipe_http_error_is_catchable() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(500, b"boom")]);
    let script = r#"
try {
  ctx.http.pipe(ctx.env.UPSTREAM_URL);
  ctx.respond(500, {}, "expected an upstream status error");
} catch (error) {
  ctx.respond(200, {}, error.code);
}
"#;
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "upstream_http_error");
}

#[test]
fn ctx_http_pipe_transport_failure_is_catchable() {
    let port = closed_port();
    let url = format!("http://127.0.0.1:{port}/file.pdf");
    let script = r#"
try {
  ctx.http.pipe(ctx.env.UPSTREAM_URL);
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
fn ctx_http_pipe_is_ignored_after_a_response_was_produced() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"%PDF")]);
    let script = r#"
ctx.respond(201, { "X-First": "yes" }, "first");
var produced = ctx.http.pipe(ctx.env.UPSTREAM_URL);
ctx.respond(500, {}, produced ? "pipe won" : "still first");
"#;
    let url = upstream.url("/file.pdf");
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 201, "body: {}", response.body);
    assert_eq!(response.body, "first");
    assert_eq!(response.header("x-first"), Some("yes"));
    assert!(stderr.contains("already produced"), "stderr: {stderr}");
    assert!(
        upstream.requests().is_empty(),
        "an ignored pipe must not reach the upstream"
    );
}

#[test]
fn ctx_http_pipe_response_wins_over_a_later_respond() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, b"%PDF-wins")]);
    let script = r#"
ctx.http.pipe(ctx.env.UPSTREAM_URL);
ctx.respond(500, {}, "second");
"#;
    let url = upstream.url("/file.pdf");
    let (response, stderr) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "%PDF-wins");
    assert!(stderr.contains("already produced"), "stderr: {stderr}");
}

#[test]
fn ctx_http_pipe_follows_allowlisted_redirects() {
    let upstream = Upstream::start_with(|port| {
        vec![
            UpstreamResponse::new(302, b"")
                .header("Location", &format!("http://127.0.0.1:{port}/final.pdf")),
            UpstreamResponse::new(200, b"%PDF-redirected"),
        ]
    });
    let script = r"ctx.http.pipe(ctx.env.UPSTREAM_URL);";
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "%PDF-redirected");
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "upstream calls: {requests:?}");
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .starts_with("get /final.pdf "),
        "unexpected upstream request: {}",
        requests[1]
    );
}

#[test]
fn ctx_http_pipe_rejects_redirect_to_non_allowlisted_host() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(302, b"")
        .header("Location", "http://not-allowed.example.com/final.pdf")]);
    let script = r#"ctx.http.pipe(ctx.env.UPSTREAM_URL); ctx.respond(200, {}, "no");"#;
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
    assert_eq!(upstream.requests().len(), 1);
}

#[test]
fn ctx_http_pipe_requires_allowlisted_host() {
    let script =
        r#"ctx.http.pipe("http://not-allowed.example.com/file.pdf"); ctx.respond(200, {}, "no");"#;
    assert_script_error(good_config(), script);
}

#[test]
fn ctx_http_pipe_defaults_to_the_upstream_2xx_status() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(207, b"%PDF-multi")]);
    let script = r"ctx.http.pipe(ctx.env.UPSTREAM_URL);";
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 207, "body: {}", response.body);
    assert_eq!(response.body, "%PDF-multi");
}

#[test]
fn ctx_http_pipe_url_rejection_is_catchable() {
    let script = r#"
try {
  ctx.http.pipe("http://files.example.com:bad/file.pdf");
  ctx.respond(500, {}, "expected an invalid-URL error");
} catch (error) {
  ctx.respond(200, {}, error.code + ":" + error.message);
}
"#;
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["files.example.com"]),
        script,
        &[],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert!(
        response.body.starts_with("upstream_url_invalid:"),
        "body: {}",
        response.body
    );
}

#[test]
fn ctx_http_pipe_redirect_limit_is_catchable() {
    let upstream = Upstream::start_with(|port| {
        (0..4_u8)
            .map(|hop| {
                UpstreamResponse::new(302, b"")
                    .header("Location", &format!("http://127.0.0.1:{port}/hop{hop}"))
            })
            .collect()
    });
    let script = r#"
try {
  ctx.http.pipe(ctx.env.UPSTREAM_URL);
  ctx.respond(500, {}, "expected a redirect error");
} catch (error) {
  ctx.respond(200, {}, error.code);
}
"#;
    let url = upstream.url("/file.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "upstream_redirect_error");
    assert_eq!(upstream.requests().len(), 4);
}

#[test]
fn ctx_http_pipe_streams_bodies_larger_than_the_get_cap() {
    let large = vec![b'a'; 8 * 1024 * 1024 + 1];
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, &large)]);
    let script = r"ctx.http.pipe(ctx.env.UPSTREAM_URL);";
    let url = upstream.url("/large.pdf");
    let (response, _) = with_server_full(
        &with_upstream(good_config(), &["127.0.0.1"]),
        script,
        &[("UPSTREAM_URL", url.as_str())],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body.len());
    assert_eq!(response.body.len(), large.len());
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
    let hosts_marker = "allow_hosts = [\"metadata.example.com\", \"files.example.com\"]";
    assert!(config.contains(port_marker), "demo config moved: {config}");
    assert!(config.contains(hosts_marker), "demo config moved: {config}");
    let config = config
        .replacen(port_marker, &format!("port = {port}"), 1)
        .replacen(
            hosts_marker,
            "allow_hosts = [\"metadata.example.com\", \"files.example.com\", \"127.0.0.1\"]",
            1,
        );
    std::fs::create_dir_all(dir.join("files")).expect("files dir");
    std::fs::create_dir_all(dir.join("scripts")).expect("scripts dir");
    for script in ["manifest.js", "download.js"] {
        std::fs::copy(
            source.join("scripts").join(script),
            dir.join("scripts").join(script),
        )
        .expect("copy demo script");
    }
    let path = dir.join("stuntdouble.toml");
    std::fs::write(&path, config).expect("write demo config");
    path
}

/// Serve a copy of the repository demo fixture with `env` set.
fn with_demo<T>(env: &[(&str, &str)], run_tests: impl FnOnce(u16) -> T) -> (T, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    serve_and_run(|port| demo_fixture(dir.path(), port), env, &[], run_tests)
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

fn demo_download_request(port: u16, document_id: &str, headers: &[(&str, &str)]) -> Response {
    request(
        port,
        "GET",
        &format!("/demo/documents/download/{document_id}"),
        headers,
    )
}

/// Run one download request against a served demo fixture whose
/// `METADATA_API_URL` points at `upstream`.
fn download_response(upstream: &Upstream, document_id: &str, headers: &[(&str, &str)]) -> Response {
    let url = upstream.url("/demo/documents");
    let (response, _) = with_demo(&[("METADATA_API_URL", url.as_str())], |port| {
        demo_download_request(port, document_id, headers)
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

/// Metadata payload for the demo fixture; `pdf_url` is embedded verbatim.
fn demo_metadata(pdf_url: &str) -> String {
    format!(
        r#"{{"data":[{{"code":"DOC-0001","title":"示例设备 A 安装手册","system_code":"SYS-A","pdf_url":"{pdf_url}"}}]}}"#
    )
}

#[test]
fn demo_download_route_streams_the_pdf() {
    const PDF: &[u8] = b"%PDF-1.4\n% demo fixture\n%%EOF\n";
    let upstream = Upstream::start_with(|port| {
        let metadata = format!(
            r#"{{"data":[{{"code":"DOC-0001","title":"示例设备 A 安装手册","system_code":"SYS-A","pdf_url":"http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf"}}]}}"#
        );
        vec![
            UpstreamResponse::new(200, metadata.as_bytes()),
            UpstreamResponse::new(200, PDF),
        ]
    });
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.header("content-type"), Some("application/pdf"));
    assert_eq!(
        response.header("content-disposition"),
        Some("attachment;filename=\"DOC-0001.pdf\"")
    );
    assert_eq!(response.body.as_bytes(), PDF);
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "upstream calls: {requests:?}");
    let head = requests[1].to_ascii_lowercase();
    assert!(
        head.starts_with("get /demo/documents/doc-0001.pdf "),
        "unexpected upstream request: {}",
        requests[1]
    );
}

#[test]
fn demo_download_route_answers_404_for_unknown_document() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(
        200,
        demo_metadata("http://files.example.com/demo/documents/DOC-0001.pdf").as_bytes(),
    )]);
    let response = download_response(&upstream, "DOC-9999", &[]);
    assert_eq!(response.status, 404, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("document_not_found")
    );
    assert_eq!(
        upstream.requests().len(),
        1,
        "a missing document must not fetch any PDF"
    );
}

#[test]
fn demo_download_route_rejects_an_invalid_pdf_url() {
    for bad_url in [
        "ftp://files.example.com/DOC-0001.pdf",
        "not-a-url",
        "files.example.com/DOC-0001.pdf",
        "http://files.example.com:bad/DOC-0001.pdf",
        "http://[::1",
        "",
    ] {
        let upstream = Upstream::start(vec![UpstreamResponse::new(
            200,
            demo_metadata(bad_url).as_bytes(),
        )]);
        let response = download_response(&upstream, "DOC-0001", &[]);
        assert_eq!(
            response.status, 502,
            "url {bad_url:?} body: {}",
            response.body
        );
        assert_eq!(
            error_class(&response.body).as_deref(),
            Some("pdf_url_invalid"),
            "url {bad_url:?} body: {}",
            response.body
        );
        assert_eq!(
            upstream.requests().len(),
            1,
            "url {bad_url:?} must not fetch any PDF"
        );
    }
}

#[test]
fn demo_download_route_maps_pdf_non_2xx_to_502() {
    let upstream = Upstream::start_with(|port| {
        let url = format!("http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf");
        vec![
            UpstreamResponse::new(200, demo_metadata(&url).as_bytes()),
            UpstreamResponse::new(503, b"unavailable"),
        ]
    });
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("pdf_bad_gateway")
    );
}

#[test]
fn demo_download_route_maps_unreachable_pdf_to_502() {
    let dead = closed_port();
    let upstream = Upstream::start(vec![UpstreamResponse::new(
        200,
        demo_metadata(&format!("http://127.0.0.1:{dead}/DOC-0001.pdf")).as_bytes(),
    )]);
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("pdf_bad_gateway")
    );
}

#[test]
fn demo_download_route_maps_metadata_failure_to_502() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(404, b"missing")]);
    assert_metadata_bad_gateway(&download_response(&upstream, "DOC-0001", &[]));
}

#[test]
fn demo_download_route_forwards_range_and_preserves_content_range() {
    const PDF: &[u8] = b"%PDF-1.4\n% demo fixture\n%%EOF\n";
    let upstream = Upstream::start_with(|port| {
        let url = format!("http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf");
        vec![
            UpstreamResponse::new(200, demo_metadata(&url).as_bytes()),
            UpstreamResponse::new(206, &PDF[..4])
                .header("Content-Range", &format!("bytes 0-3/{}", PDF.len())),
        ]
    });
    let response = download_response(&upstream, "DOC-0001", &[("Range", "bytes=0-3")]);
    assert_eq!(response.status, 206, "body: {}", response.body);
    assert_eq!(response.body.as_bytes(), &PDF[..4]);
    assert_eq!(
        response.header("content-range"),
        Some(format!("bytes 0-3/{}", PDF.len()).as_str())
    );
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "upstream calls: {requests:?}");
    let head = requests[1].to_ascii_lowercase();
    assert!(
        head.contains("range: bytes=0-3"),
        "Range header not forwarded: {}",
        requests[1]
    );
}

#[test]
fn demo_download_route_keeps_policy_rejections_as_script_errors() {
    let upstream = Upstream::start(vec![UpstreamResponse::new(
        200,
        demo_metadata("http://not-allowed.example.com/DOC-0001.pdf").as_bytes(),
    )]);
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn demo_download_route_maps_unfollowable_redirects_to_502() {
    let upstream = Upstream::start_with(|port| {
        let url = format!("http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf");
        let mut responses = vec![UpstreamResponse::new(200, demo_metadata(&url).as_bytes())];
        for hop in 0..4_u8 {
            responses.push(
                UpstreamResponse::new(302, b"")
                    .header("Location", &format!("http://127.0.0.1:{port}/hop{hop}")),
            );
        }
        responses
    });
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("pdf_bad_gateway")
    );
    assert_eq!(upstream.requests().len(), 5);
}

#[test]
fn demo_download_route_maps_a_redirect_without_location_to_502() {
    let upstream = Upstream::start_with(|port| {
        let url = format!("http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf");
        vec![
            UpstreamResponse::new(200, demo_metadata(&url).as_bytes()),
            UpstreamResponse::new(302, b""),
        ]
    });
    let response = download_response(&upstream, "DOC-0001", &[]);
    assert_eq!(response.status, 502, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("pdf_bad_gateway")
    );
}

#[test]
fn demo_download_route_does_not_forward_client_request_id() {
    let upstream = Upstream::start_with(|port| {
        let url = format!("http://127.0.0.1:{port}/demo/documents/DOC-0001.pdf");
        vec![
            UpstreamResponse::new(200, demo_metadata(&url).as_bytes()),
            UpstreamResponse::new(200, b"%PDF-1.4\n%%EOF\n"),
        ]
    });
    let response = download_response(&upstream, "DOC-0001", &[("X-Request-ID", "key")]);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert!(response.header("x-request-id").is_some());
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "upstream calls: {requests:?}");
    for head in &requests {
        let lower = head.to_ascii_lowercase();
        // Positive control: the capture holds a real request head, so the
        // negative assertion below cannot pass on an empty recording.
        assert!(lower.contains("host:"), "recorded head: {head}");
        assert!(
            !lower.contains("x-request-id"),
            "client header leaked upstream: {head}"
        );
    }
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

#[cfg(unix)]
#[derive(Clone, Copy)]
enum SignalCase {
    Interrupt,
    Terminate,
}

#[cfg(unix)]
impl SignalCase {
    fn kill_arg(self) -> &'static str {
        match self {
            Self::Interrupt => "INT",
            Self::Terminate => "TERM",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    fn forced_exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

/// A served fixture whose route waits on a delayed upstream response.
#[cfg(unix)]
struct InFlightFixture {
    upstream: Upstream,
    child: Child,
    log: PathBuf,
    port: u16,
    _dir: tempfile::TempDir,
}

#[cfg(unix)]
fn start_in_flight_request(delay: Duration, body: &[u8]) -> InFlightFixture {
    let upstream = Upstream::start(vec![UpstreamResponse::new(200, body).delay(delay)]);
    let config_body = with_upstream(good_config(), &["127.0.0.1"]);
    let script = format!(
        r#"var r = ctx.http.get("{url}");
ctx.respond(200, {{ "Content-Type": "text/plain; charset=utf-8" }}, r.text());
"#,
        url = upstream.url("/slow")
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let (child, log, port) = start_serve(
        |port| fixture_with_script(dir.path(), port, &config_body, &script),
        &[],
        &[],
    );
    InFlightFixture {
        upstream,
        child,
        log,
        port,
        _dir: dir,
    }
}

#[cfg(unix)]
fn send_signal(child: &Child, signal: &str) {
    let pid = child.id().to_string();
    let status = Command::new("kill")
        .args([format!("-{signal}"), pid.clone()])
        .status()
        .expect("run kill");
    assert!(
        status.success(),
        "kill -{signal} {pid} failed with {status}"
    );
}

#[cfg(unix)]
fn wait_for_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_for_upstream_request(upstream: &Upstream) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.requests().is_empty() {
        assert!(
            Instant::now() < deadline,
            "upstream did not receive a request in time"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_for_log(log: &Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "log did not contain {needle:?}: {text}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn assert_first_signal_exits(signal: SignalCase) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut child, log, _) =
        start_serve(|port| fixture(dir.path(), port, good_config()), &[], &[]);

    send_signal(&child, signal.kill_arg());
    let status = wait_for_exit(&mut child, Duration::from_secs(5));
    let stderr = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        status.code(),
        Some(0),
        "status: {status:?}, stderr: {stderr}"
    );
    let graceful = format!("{} received; starting graceful shutdown", signal.name());
    assert!(stderr.contains(&graceful), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn sigterm_triggers_graceful_shutdown_with_exit_code_zero() {
    assert_first_signal_exits(SignalCase::Terminate);
}

#[cfg(unix)]
#[test]
fn sigint_triggers_graceful_shutdown_with_exit_code_zero() {
    assert_first_signal_exits(SignalCase::Interrupt);
}

#[cfg(unix)]
fn assert_drain_preserves_in_flight_request(signal: SignalCase) {
    let mut fixture = start_in_flight_request(Duration::from_millis(500), b"drained");
    let port = fixture.port;
    let request_thread =
        std::thread::spawn(move || request(port, "GET", "/demo/documents/manifest/group-a", &[]));
    wait_for_upstream_request(&fixture.upstream);
    send_signal(&fixture.child, signal.kill_arg());

    let response = request_thread.join().expect("request thread");
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "drained");
    let status = wait_for_exit(&mut fixture.child, Duration::from_secs(5));
    let stderr = std::fs::read_to_string(&fixture.log).unwrap_or_default();
    assert_eq!(
        status.code(),
        Some(0),
        "status: {status:?}, stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn sigterm_drains_in_flight_requests() {
    assert_drain_preserves_in_flight_request(SignalCase::Terminate);
}

#[cfg(unix)]
#[test]
fn sigint_drains_in_flight_requests() {
    assert_drain_preserves_in_flight_request(SignalCase::Interrupt);
}

#[cfg(unix)]
fn assert_second_signal_forces_exit(signal: SignalCase) {
    let mut fixture = start_in_flight_request(Duration::from_secs(30), b"late");
    let mut client = TcpStream::connect(("127.0.0.1", fixture.port)).expect("connect");
    let head = "GET /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    client.write_all(head.as_bytes()).expect("write request");
    client.flush().expect("flush");
    wait_for_upstream_request(&fixture.upstream);

    send_signal(&fixture.child, signal.kill_arg());
    let started = format!("{} received; starting graceful shutdown", signal.name());
    wait_for_log(&fixture.log, &started);
    assert!(
        fixture.child.try_wait().expect("try_wait").is_none(),
        "server exited before the second signal"
    );

    send_signal(&fixture.child, signal.kill_arg());
    let status = wait_for_exit(&mut fixture.child, Duration::from_secs(5));
    let stderr = std::fs::read_to_string(&fixture.log).unwrap_or_default();
    assert_eq!(
        status.code(),
        Some(signal.forced_exit_code()),
        "status: {status:?}, stderr: {stderr}"
    );
    let forced = format!("{} received again; terminating immediately", signal.name());
    assert!(stderr.contains(&forced), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn second_sigterm_terminates_immediately_with_exit_code_143() {
    assert_second_signal_forces_exit(SignalCase::Terminate);
}

#[cfg(unix)]
#[test]
fn second_sigint_terminates_immediately_with_exit_code_130() {
    assert_second_signal_forces_exit(SignalCase::Interrupt);
}

/// Back-to-back signals may be coalesced by the OS. Either the second signal is
/// observed (exit 143) or the first signal drains normally (exit 0); neither
/// path may hang.
#[cfg(unix)]
#[test]
fn back_to_back_sigterm_signals_still_exit() {
    let mut fixture = start_in_flight_request(Duration::from_secs(1), b"late");
    let mut client = TcpStream::connect(("127.0.0.1", fixture.port)).expect("connect");
    let head = "GET /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    client.write_all(head.as_bytes()).expect("write request");
    client.flush().expect("flush");
    wait_for_upstream_request(&fixture.upstream);

    send_signal(&fixture.child, SignalCase::Terminate.kill_arg());
    send_signal(&fixture.child, SignalCase::Terminate.kill_arg());
    let status = wait_for_exit(&mut fixture.child, Duration::from_secs(5));
    let stderr = std::fs::read_to_string(&fixture.log).unwrap_or_default();
    assert!(
        matches!(status.code(), Some(0 | 143)),
        "status: {status:?}, stderr: {stderr}"
    );
}
