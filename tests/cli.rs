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
    body_bytes: Vec<u8>,
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
    request_bytes(port, method, path, headers, body.as_bytes())
}

fn request_bytes(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut head = String::new();
    let _ = write!(
        head,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
    stream.flush().expect("flush");
    read_response(&mut stream)
}

/// Send a request head plus an incomplete body, then read the answer without
/// ever finishing the announced body.
fn request_partial_body(
    port: u16,
    path: &str,
    content_type: &str,
    announced: usize,
    partial: &[u8],
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {announced}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(partial).expect("write partial body");
    stream.flush().expect("flush");
    read_response_until_complete(&mut stream)
}

/// Give a fixture config a short request deadline.
fn with_request_timeout(body: &str, timeout_ms: u64) -> String {
    body.replace(
        "port = {port}",
        &format!("port = {{port}}\nrequest_timeout_ms = {timeout_ms}"),
    )
}

/// The same deadline plus a POST route, so a test can send an unfinished body
/// to a matched route.
fn with_partial_body_route(body: &str, timeout_ms: u64) -> String {
    format!(
        "{}\n[[routes]]\nname = \"partial-body\"\nmethod = \"POST\"\npath = \"/demo/documents/manifest/group-a\"\nscript = \"scripts/manifest.js\"\n",
        with_request_timeout(body, timeout_ms)
    )
}

/// One part in a test-built multipart body. `filename` distinguishes a file
/// field from a non-file form field.
struct MultipartPart<'a> {
    name: Option<&'a str>,
    filename: Option<&'a str>,
    content_type: Option<&'a str>,
    data: &'a [u8],
}

impl<'a> MultipartPart<'a> {
    fn field(name: &'a str, data: &'a [u8]) -> Self {
        Self {
            name: Some(name),
            filename: None,
            content_type: None,
            data,
        }
    }

    fn file(name: &'a str, filename: &'a str, content_type: &'a str, data: &'a [u8]) -> Self {
        Self {
            name: Some(name),
            filename: Some(filename),
            content_type: Some(content_type),
            data,
        }
    }

    fn unnamed_file(filename: &'a str, content_type: &'a str, data: &'a [u8]) -> Self {
        Self {
            name: None,
            filename: Some(filename),
            content_type: Some(content_type),
            data,
        }
    }
}

fn multipart_body(boundary: &str, parts: &[MultipartPart<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let mut disposition = String::from("Content-Disposition: form-data");
        if let Some(name) = part.name {
            let _ = write!(disposition, "; name=\"{name}\"");
        }
        if let Some(filename) = part.filename {
            let _ = write!(disposition, "; filename=\"{filename}\"");
        }
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(b"\r\n");
        if let Some(content_type) = part.content_type {
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(part.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn request_multipart(
    port: u16,
    path: &str,
    boundary: &str,
    parts: &[MultipartPart<'_>],
) -> Response {
    let body = multipart_body(boundary, parts);
    let content_type = format!("multipart/form-data; boundary={boundary}");
    request_bytes(
        port,
        "POST",
        path,
        &[("Content-Type", content_type.as_str())],
        &body,
    )
}

fn request_multipart_with_headers(
    port: u16,
    path: &str,
    boundary: &str,
    parts: &[MultipartPart<'_>],
    headers: &[(&str, &str)],
) -> Response {
    let body = multipart_body(boundary, parts);
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let mut all_headers = vec![("Content-Type", content_type.as_str())];
    all_headers.extend_from_slice(headers);
    request_bytes(port, "POST", path, &all_headers, &body)
}

fn abort_multipart_after_response_head(port: u16, path: &str, boundary: &str, body: &[u8]) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(body).expect("write body");
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

fn request_chunked_bytes(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    chunks: &[&[u8]],
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut head = String::new();
    let _ = write!(
        head,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n"
    );
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        let prefix = format!("{:x}\r\n", chunk.len());
        let _ = stream.write_all(prefix.as_bytes());
        let _ = stream.write_all(chunk);
        let _ = stream.write_all(b"\r\n");
    }
    let _ = stream.write_all(b"0\r\n\r\n");
    let _ = stream.flush();
    read_response(&mut stream)
}

fn request_multipart_chunked(
    port: u16,
    path: &str,
    boundary: &str,
    parts: &[MultipartPart<'_>],
) -> Response {
    let body = multipart_body(boundary, parts);
    let split = body.len() / 2;
    let content_type = format!("multipart/form-data; boundary={boundary}");
    request_chunked_bytes(
        port,
        "POST",
        path,
        &[("Content-Type", content_type.as_str())],
        &[&body[..split], &body[split..]],
    )
}

/// Send only the request head with a declared `Content-Length`, so a
/// Content-Length pre-check can answer without waiting for the body.
fn request_with_declared_length(
    port: u16,
    path: &str,
    content_type: &str,
    declared: u64,
) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {declared}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).expect("write head");
    stream.flush().expect("flush");
    read_response(&mut stream)
}

/// Per-test temp root for server-created upload directories.
fn upload_temp_root(dir: &Path) -> PathBuf {
    let root = dir.join("uploads-tmp");
    std::fs::create_dir_all(&root).expect("upload temp root");
    root
}

fn temp_entries(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .expect("read upload temp root")
        .map(|entry| entry.expect("temp entry").path())
        .collect()
}

fn wait_for_temp_entries(root: &Path) -> Vec<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = temp_entries(root);
        if !entries.is_empty() {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "upload temp directory never appeared under {}",
            root.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_empty_temp(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = temp_entries(root);
        if entries.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "upload temp entries were not removed: {entries:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Read one full HTTP/1.1 response from `stream`.
fn read_response(stream: &mut TcpStream) -> Response {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).expect("read response");
    parse_response(&bytes)
}

/// Read one complete response without waiting for EOF, so a test that leaves a
/// request body unfinished can still assert on the server's answer.
fn read_response_until_complete(stream: &mut TcpStream) -> Response {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let mut expected_body = None;
    loop {
        if let Some(head_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let body_len = *expected_body.get_or_insert_with(|| {
                String::from_utf8_lossy(&bytes[..head_end])
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0)
            });
            if bytes.len() >= head_end + 4 + body_len {
                return parse_response(&bytes);
            }
        }
        let read = stream.read(&mut buffer).expect("read response");
        assert_ne!(read, 0, "server closed before the response completed");
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn parse_response(bytes: &[u8]) -> Response {
    let head_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(bytes.len());
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let body_bytes = bytes.get(head_end + 4..).unwrap_or_default().to_vec();
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
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
        body_bytes,
    }
}

/// Send one GET with a raw Range field value, so tests can carry header bytes
/// that are not valid UTF-8.
fn request_raw_range(port: u16, path: &str, range: &[u8]) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut raw = Vec::new();
    let _ = write!(
        raw,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nRange: "
    );
    raw.extend_from_slice(range);
    raw.extend_from_slice(b"\r\n\r\n");
    stream.write_all(&raw).expect("write request");
    stream.flush().expect("flush");
    read_response(&mut stream)
}

/// Send one request and drop the connection after reading the response head
/// plus one body byte, so the server observes a client that left mid-body.
fn abort_after_response_head(port: u16, path: &str) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let raw = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
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
fn validate_rejects_a_non_positive_server_request_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = with_request_timeout(good_config(), 0);
    let config = fixture(dir.path(), 3000, &broken);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "expected configuration error, stderr: {stderr}");
    assert!(
        stderr.contains(
            "server.request_timeout_ms: expected integer number of milliseconds greater than 0"
        ),
        "stderr: {stderr}"
    );
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
fn validate_accepts_files_upload_max_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let configured = good_config().replace(
        "[files]\nroot = \"./files\"",
        "[files]\nroot = \"./files\"\nupload_max_bytes = 1024",
    );
    let config = fixture(dir.path(), 3000, &configured);
    let (code, stdout, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("valid configuration"), "stdout: {stdout}");
}

#[test]
fn validate_rejects_invalid_files_upload_max_bytes() {
    for value in ["0", "-1", "1.5", "\"1024\""] {
        let dir = tempfile::tempdir().expect("tempdir");
        let configured = good_config().replace(
            "[files]\nroot = \"./files\"",
            &format!("[files]\nroot = \"./files\"\nupload_max_bytes = {value}"),
        );
        let config = fixture(dir.path(), 3000, &configured);
        let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
        assert_eq!(code, 2, "value {value} stderr: {stderr}");
        assert!(
            stderr.contains("files.upload_max_bytes"),
            "value {value} stderr: {stderr}"
        );
    }
}

#[test]
fn validate_rejects_unknown_files_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let configured = good_config().replace(
        "[files]\nroot = \"./files\"",
        "[files]\nroot = \"./files\"\nbogus = true",
    );
    let config = fixture(dir.path(), 3000, &configured);
    let (code, _, stderr) = run(&["validate", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("files.bogus"), "stderr: {stderr}");
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

/// Serve the standard fixture Route with `files/*` fixtures that the caller
/// wrote under `dir` beforehand; the config is rewritten per attempt port.
fn serve_fixture_dir<T>(dir: &Path, script: &str, run_tests: impl FnOnce(u16) -> T) -> (T, String) {
    serve_and_run(
        |port| fixture_with_script(dir, port, good_config(), script),
        &[],
        &[],
        run_tests,
    )
}

fn upload_config(upload_max_bytes: u64) -> String {
    good_config()
        .replace("method = \"GET\"", "method = \"POST\"")
        .replace(
            "[files]\nroot = \"./files\"",
            &format!("[files]\nroot = \"./files\"\nupload_max_bytes = {upload_max_bytes}"),
        )
}

fn serve_upload_fixture<T>(
    dir: &Path,
    config: &str,
    script: &str,
    env: &[(&str, &str)],
    run_tests: impl FnOnce(u16) -> T,
) -> (T, String) {
    serve_and_run(
        |port| fixture_with_script(dir, port, config, script),
        env,
        &[],
        run_tests,
    )
}

#[test]
fn multipart_upload_exposes_file_metadata_and_contents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
var file = ctx.request.files[0];
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify({
  count: ctx.request.files.length,
  field: file.field,
  filename: file.filename,
  contentType: file.contentType,
  size: file.size,
  bodyText: ctx.request.bodyText,
  filesFrozen: Object.isFrozen(ctx.request.files),
  fileFrozen: Object.isFrozen(file),
  text: file.text(),
  textAgain: file.text(),
  bytes: Array.prototype.slice.call(file.bytes())
}));
"#;
    let (response, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            request_multipart(
                port,
                "/demo/documents/manifest/group-a",
                "sd-upload-boundary",
                &[
                    MultipartPart::file(
                        "document",
                        "../uploads/report.txt",
                        "text/plain",
                        b"hello upload",
                    ),
                    MultipartPart::field("note", b"ignored"),
                ],
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["count"], 1);
    assert_eq!(json["field"], "document");
    assert_eq!(json["filename"], "report.txt");
    assert_eq!(json["contentType"], "text/plain");
    assert_eq!(json["size"], 12);
    assert_eq!(json["bodyText"], serde_json::Value::Null);
    assert_eq!(json["filesFrozen"], true);
    assert_eq!(json["fileFrozen"], true);
    assert_eq!(json["text"], "hello upload");
    assert_eq!(json["textAgain"], "hello upload");
    assert_eq!(
        json["bytes"],
        serde_json::json!([104, 101, 108, 108, 111, 32, 117, 112, 108, 111, 97, 100])
    );
}

#[test]
fn multipart_limit_accepts_the_cap_and_rejects_precheck_chunked_and_form_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
var file = ctx.request.files[0];
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify({
  size: file.size,
  text: file.text()
}));
"#;
    let limit = 64_u64;
    let (responses, stderr) =
        serve_upload_fixture(dir.path(), &upload_config(limit), script, &[], |port| {
            let path = "/demo/documents/manifest/group-a";
            let at_cap = request_multipart(
                port,
                path,
                "sd-at-cap",
                &[MultipartPart::file(
                    "document",
                    "at-cap.bin",
                    "application/octet-stream",
                    &[b'a'; 64],
                )],
            );
            let over_limit = request_multipart(
                port,
                path,
                "sd-over-limit",
                &[MultipartPart::file(
                    "document",
                    "over-limit.bin",
                    "application/octet-stream",
                    &[b'a'; 65],
                )],
            );
            let chunked = request_multipart_chunked(
                port,
                path,
                "sd-chunked",
                &[MultipartPart::file(
                    "document",
                    "chunked.bin",
                    "application/octet-stream",
                    &[b'a'; 65],
                )],
            );
            let form_field = request_multipart(
                port,
                path,
                "sd-form-field",
                &[MultipartPart::field("note", &[b'a'; 65])],
            );
            let precheck = request_with_declared_length(
                port,
                path,
                "multipart/form-data; boundary=sd-precheck",
                limit + 1024 * 1024 + 1,
            );
            [at_cap, over_limit, chunked, form_field, precheck]
        });
    assert_eq!(responses[0].status, 200, "body: {}", responses[0].body);
    let json: serde_json::Value = serde_json::from_str(&responses[0].body).expect("json body");
    assert_eq!(json["size"], 64);
    assert_eq!(json["text"].as_str().map(str::len), Some(64));
    for response in &responses[1..] {
        assert_eq!(response.status, 413, "body: {}", response.body);
        assert_eq!(
            error_class(&response.body).as_deref(),
            Some("upload_too_large"),
            "body: {}",
            response.body
        );
    }
    let logs: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .collect();
    assert_eq!(logs.len(), 5, "stderr: {stderr}");
    assert_eq!(logs[0]["upload"]["files"], 1);
    assert_eq!(logs[0]["upload"]["total_bytes"], 64);
    assert_eq!(logs[0]["upload"]["error"], serde_json::Value::Null);
    for log in &logs[1..] {
        assert_eq!(log["upload"]["error"], "upload_too_large", "log: {log}");
    }
}

#[test]
fn malformed_multipart_maps_to_400_before_the_script_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.log.info("script-ran");
ctx.respond(200, {}, "unreachable");
"#;
    let config = upload_config(1024 * 1024);
    let (responses, stderr) = serve_and_run(
        |port| fixture_with_script(dir.path(), port, &config, script),
        &[],
        &["--verbose"],
        |port| {
            let path = "/demo/documents/manifest/group-a";
            [
                request_bytes(
                    port,
                    "POST",
                    path,
                    &[("Content-Type", "multipart/form-data; boundary=sd-malformed")],
                    b"not a multipart body",
                ),
                request_bytes(
                    port,
                    "POST",
                    path,
                    &[("Content-Type", "multipart/form-data")],
                    b"missing boundary",
                ),
            ]
        },
    );
    for response in &responses {
        assert_eq!(response.status, 400, "body: {}", response.body);
        assert_eq!(
            error_class(&response.body).as_deref(),
            Some("invalid_multipart"),
            "body: {}",
            response.body
        );
        assert!(
            json_string(&response.body, "request_id").is_some(),
            "body: {}",
            response.body
        );
    }
    assert!(!stderr.contains("script-ran"), "stderr: {stderr}");
    assert!(!stderr.contains("not a multipart body"), "stderr: {stderr}");
    assert!(
        responses[0].body.contains("multipart body is malformed"),
        "body: {}",
        responses[0].body
    );
    let logs: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .collect();
    assert_eq!(logs.len(), 2, "stderr: {stderr}");
    for log in logs {
        assert_eq!(log["status"], 400);
        assert_eq!(log["error"], "invalid_multipart");
        assert_eq!(log["upload"]["error"], "invalid_multipart");
    }
}

#[test]
fn multipart_part_without_name_is_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.log.info("script-ran");
ctx.respond(200, {}, "unreachable");
"#;
    let (response, stderr) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            request_multipart(
                port,
                "/demo/documents/manifest/group-a",
                "sd-unnamed",
                &[MultipartPart::unnamed_file(
                    "x.bin",
                    "application/octet-stream",
                    b"x",
                )],
            )
        },
    );
    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("invalid_multipart")
    );
    assert!(!stderr.contains("script-ran"), "stderr: {stderr}");
    let log: serde_json::Value = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .expect("request log");
    assert_eq!(log["error"], "invalid_multipart");
    assert_eq!(log["upload"]["error"], "invalid_multipart");
}

#[test]
fn chunked_invalid_boundary_logs_unknown_request_body_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.respond(200, {}, "unreachable");
"#;
    let (response, stderr) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            request_chunked_bytes(
                port,
                "POST",
                "/demo/documents/manifest/group-a",
                &[("Content-Type", "multipart/form-data")],
                &[b"x"],
            )
        },
    );
    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("invalid_multipart")
    );
    let log: serde_json::Value = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .expect("request log");
    assert_eq!(
        log["request_body_bytes"],
        serde_json::Value::Null,
        "log: {log}"
    );
    assert_eq!(log["upload"]["total_bytes"], 0);
    assert_eq!(log["upload"]["error"], "invalid_multipart");
}

#[test]
fn upload_text_bytes_are_strict_utf8_and_capped_at_eight_mib() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
var file = ctx.request.files[0];
function attempt(fn) {
  try { return fn(); } catch (error) { return error.code; }
}
var out = { text: attempt(function () { return file.text(); }) };
out.bytes = file.size <= 16
  ? attempt(function () { return Array.prototype.slice.call(file.bytes()); })
  : "skipped";
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify(out));
"#;
    let cap = 8 * 1024 * 1024;
    let (responses, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(9 * 1024 * 1024),
        script,
        &[],
        |port| {
            let path = "/demo/documents/manifest/group-a";
            let invalid = request_multipart(
                port,
                path,
                "sd-encoding",
                &[MultipartPart::file(
                    "document",
                    "latin1.bin",
                    "application/octet-stream",
                    &[0x66, 0x6f, 0x80],
                )],
            );
            let over_cap = request_multipart(
                port,
                path,
                "sd-over-cap",
                &[MultipartPart::file(
                    "document",
                    "over-cap.bin",
                    "application/octet-stream",
                    &vec![b'a'; cap + 1],
                )],
            );
            [invalid, over_cap]
        },
    );
    assert_eq!(responses[0].status, 200, "body: {}", responses[0].body);
    let json: serde_json::Value = serde_json::from_str(&responses[0].body).expect("json body");
    assert_eq!(json["text"], "file_encoding_error");
    assert_eq!(json["bytes"], serde_json::json!([102, 111, 128]));
    assert_eq!(responses[1].status, 200, "body: {}", responses[1].body);
    let json: serde_json::Value = serde_json::from_str(&responses[1].body).expect("json body");
    assert_eq!(json["text"], "file_too_large");
    assert_eq!(json["bytes"], "skipped");
}

#[test]
fn upload_filename_strips_unix_and_windows_path_components() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify(
  ctx.request.files.map(function (file) { return file.filename; })
));
"#;
    let (response, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            request_multipart(
                port,
                "/demo/documents/manifest/group-a",
                "sd-filename-boundary",
                &[
                    MultipartPart::file("unix", "dir/sub/report.txt", "text/plain", b"a"),
                    MultipartPart::file("windows", "C:\\uploads\\report.txt", "text/plain", b"b"),
                ],
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, r#"["report.txt","report.txt"]"#);
}

#[test]
fn non_multipart_body_limit_stays_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.respond(200, {}, "unreachable");
"#;
    let (response, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            let body = vec![b'a'; 2 * 1024 * 1024 + 1];
            request_bytes(
                port,
                "POST",
                "/demo/documents/manifest/group-a",
                &[("Content-Type", "text/plain")],
                &body,
            )
        },
    );
    assert_eq!(response.status, 413, "body: {}", response.body);
    assert!(
        response.body_bytes.is_empty(),
        "body: {:?}",
        response.body_bytes
    );
}

#[test]
fn chunked_multipart_framing_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.respond(200, {}, "unreachable");
"#;
    let (response, stderr) =
        serve_upload_fixture(dir.path(), &upload_config(64), script, &[], |port| {
            let oversized_name = "n".repeat(1024 * 1024 + 64);
            request_multipart_chunked(
                port,
                "/demo/documents/manifest/group-a",
                "sd-framing",
                &[MultipartPart::field(&oversized_name, b"x")],
            )
        });
    assert_eq!(response.status, 413, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("upload_too_large"),
        "body: {}",
        response.body
    );
    assert!(stderr.contains("\"upload_too_large\""), "stderr: {stderr}");
}

#[test]
fn upload_stream_reuses_file_range_and_framing_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
var file = ctx.request.files[0];
var mode = ctx.request.query.mode;
if (mode === "bad-status") {
  try { ctx.respond(201, {}, file.stream()); }
  catch (error) { ctx.respond(200, {}, error.code); }
} else if (mode === "bad-header") {
  try { ctx.respond(200, { "Content-Length": "1" }, file.stream()); }
  catch (error) { ctx.respond(200, {}, error.code); }
} else {
  ctx.respond(200, {}, file.stream());
}
"#;
    let (responses, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            let path = "/demo/documents/manifest/group-a";
            let parts = [MultipartPart::file(
                "document",
                "digits.bin",
                "application/octet-stream",
                b"0123456789",
            )];
            let full = request_multipart(port, path, "sd-full", &parts);
            let partial = request_multipart_with_headers(
                port,
                path,
                "sd-range",
                &parts,
                &[("Range", "bytes=0-3")],
            );
            let unsatisfiable = request_multipart_with_headers(
                port,
                path,
                "sd-range-bad",
                &parts,
                &[("Range", "bytes=99-")],
            );
            let bad_status = request_multipart(
                port,
                &format!("{path}?mode=bad-status"),
                "sd-bad-status",
                &parts,
            );
            let bad_header = request_multipart(
                port,
                &format!("{path}?mode=bad-header"),
                "sd-bad-header",
                &parts,
            );
            [full, partial, unsatisfiable, bad_status, bad_header]
        },
    );
    assert_eq!(responses[0].status, 200, "body: {}", responses[0].body);
    assert_eq!(responses[0].body, "0123456789");
    assert_eq!(responses[0].header("content-length"), Some("10"));
    assert_eq!(responses[0].header("accept-ranges"), Some("bytes"));

    assert_eq!(responses[1].status, 206, "body: {}", responses[1].body);
    assert_eq!(responses[1].body, "0123");
    assert_eq!(responses[1].header("content-range"), Some("bytes 0-3/10"));
    assert_eq!(responses[1].header("content-length"), Some("4"));

    assert_eq!(responses[2].status, 416, "body: {}", responses[2].body);
    assert!(responses[2].body_bytes.is_empty());
    assert_eq!(responses[2].header("content-range"), Some("bytes */10"));
    assert_eq!(responses[2].header("content-length"), Some("0"));

    assert_eq!(responses[3].status, 200, "body: {}", responses[3].body);
    assert_eq!(responses[3].body, "script_error");
    assert_eq!(responses[4].status, 200, "body: {}", responses[4].body);
    assert_eq!(responses[4].body, "script_error");
}

#[test]
fn upload_calls_and_summary_are_logged_without_names_or_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let temp_root = upload_temp_root(dir.path());
    let temp_env = temp_root.to_string_lossy().into_owned();
    let script = r#"
ctx.request.files[0].text();
ctx.respond(200, { "Content-Type": "text/plain" }, ctx.request.files[0].stream());
"#;
    let (response, stderr) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[
            ("TMPDIR", temp_env.as_str()),
            ("TMP", temp_env.as_str()),
            ("TEMP", temp_env.as_str()),
        ],
        |port| {
            request_multipart(
                port,
                "/demo/documents/manifest/group-a",
                "sd-log",
                &[MultipartPart::file(
                    "document",
                    "client-secret-name.txt",
                    "text/plain",
                    b"four",
                )],
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "four");
    let logs: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .collect();
    assert_eq!(logs.len(), 1, "stderr: {stderr}");
    let log = &logs[0];
    assert_eq!(log["upload"]["files"], 1);
    assert_eq!(log["upload"]["total_bytes"], 4);
    assert_eq!(log["upload"]["error"], serde_json::Value::Null);
    let calls = log["file_calls"].as_array().expect("file_calls array");
    assert_eq!(calls.len(), 2, "file_calls: {calls:?}");
    assert_eq!(calls[0]["api"], "upload.text");
    assert_eq!(calls[0]["bytes"], 4);
    assert_eq!(calls[0]["error"], serde_json::Value::Null);
    assert_eq!(calls[1]["api"], "upload.stream");
    assert_eq!(calls[1]["bytes"], 4);
    assert_eq!(calls[1]["error"], serde_json::Value::Null);
    assert!(
        !stderr.contains("client-secret-name.txt"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains(&temp_root.display().to_string()),
        "stderr: {stderr}"
    );
}

#[test]
fn upload_client_filename_is_never_used_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let temp_root = upload_temp_root(dir.path());
    let temp_env = temp_root.to_string_lossy().into_owned();
    let script = r"
ctx.respond(200, {}, ctx.request.files[0].stream());
";
    let ((), stderr) = serve_upload_fixture(
        dir.path(),
        &upload_config(2 * 1024 * 1024),
        script,
        &[
            ("TMPDIR", temp_env.as_str()),
            ("TMP", temp_env.as_str()),
            ("TEMP", temp_env.as_str()),
        ],
        |port| {
            let data = vec![b'x'; 1024 * 1024];
            let body = multipart_body(
                "sd-name",
                &[MultipartPart::file(
                    "document",
                    "forbidden-client-name.bin",
                    "application/octet-stream",
                    &data,
                )],
            );
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let client = std::thread::spawn(move || {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("read timeout");
                let head = format!(
                    "POST /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary=sd-name\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).expect("write head");
                stream.write_all(&body).expect("write body");
                stream.flush().expect("flush");
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).expect("read response");
                    assert_ne!(read, 0, "server closed before the response body started");
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(head_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        if bytes.len() > head_end + 4 {
                            break;
                        }
                    }
                }
                ready_tx.send(()).expect("signal response body");
                // Keep the response open so the server-side relay blocks and
                // the request-scoped directory stays observable.
                std::thread::sleep(Duration::from_secs(2));
            });
            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("response body did not start");
            let dirs = wait_for_temp_entries(&temp_root);
            assert_eq!(dirs.len(), 1, "upload dirs: {dirs:?}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                let mode = std::fs::metadata(&dirs[0])
                    .expect("upload dir metadata")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o700, "upload directory mode: {mode:o}");
            }
            let files = temp_entries(&dirs[0]);
            assert!(!files.is_empty(), "no files under {}", dirs[0].display());
            for file in files {
                let name = file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("temp filename");
                assert!(
                    name.bytes().all(|byte| byte.is_ascii_digit()),
                    "temporary files must use opaque numbered names, got {name:?}"
                );
                assert_ne!(name, "forbidden-client-name.bin");
            }
            client.join().expect("client thread");
            wait_for_empty_temp(&temp_root);
        },
    );
    assert!(
        !stderr.contains("forbidden-client-name.bin"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains(&temp_root.display().to_string()),
        "stderr: {stderr}"
    );
}

#[test]
fn upload_temp_directories_are_removed_on_every_exit_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let temp_root = upload_temp_root(dir.path());
    let temp_env = temp_root.to_string_lossy().into_owned();
    let config = with_sandbox(&upload_config(1024 * 1024), 150);
    let script = r#"
var mode = ctx.request.query.mode;
if (mode === "error") { throw new Error("boom"); }
if (mode === "timeout") { while (true) {} }
var file = ctx.request.files[0];
if (mode === "stream") { ctx.respond(200, {}, file.stream()); }
else { ctx.respond(200, {}, file.text()); }
"#;
    let (responses, _) = serve_upload_fixture(
        dir.path(),
        &config,
        script,
        &[
            ("TMPDIR", temp_env.as_str()),
            ("TMP", temp_env.as_str()),
            ("TEMP", temp_env.as_str()),
        ],
        |port| {
            let path = "/demo/documents/manifest/group-a";
            let small = [MultipartPart::file(
                "document",
                "small.bin",
                "text/plain",
                b"abc",
            )];
            let buffered = request_multipart(port, path, "sd-clean-buffered", &small);
            wait_for_empty_temp(&temp_root);

            let script_error = request_multipart(
                port,
                &format!("{path}?mode=error"),
                "sd-clean-error",
                &small,
            );
            wait_for_empty_temp(&temp_root);

            let streamed = request_multipart(
                port,
                &format!("{path}?mode=stream"),
                "sd-clean-stream",
                &small,
            );
            wait_for_empty_temp(&temp_root);

            let malformed = request_bytes(
                port,
                "POST",
                path,
                &[("Content-Type", "multipart/form-data; boundary=sd-malformed")],
                b"not multipart",
            );
            wait_for_empty_temp(&temp_root);

            let big = vec![b'x'; 1024 * 1024];
            let body = multipart_body(
                "sd-disconnect",
                &[MultipartPart::file(
                    "document",
                    "big.bin",
                    "application/octet-stream",
                    &big,
                )],
            );
            abort_multipart_after_response_head(
                port,
                &format!("{path}?mode=stream"),
                "sd-disconnect",
                &body,
            );
            wait_for_empty_temp(&temp_root);

            let timeout = request_multipart(
                port,
                &format!("{path}?mode=timeout"),
                "sd-clean-timeout",
                &small,
            );
            wait_for_empty_temp(&temp_root);

            [buffered, script_error, streamed, malformed, timeout]
        },
    );
    assert_eq!(responses[0].status, 200, "body: {}", responses[0].body);
    assert_eq!(responses[0].body, "abc");
    assert_eq!(responses[1].status, 500, "body: {}", responses[1].body);
    assert_eq!(
        error_class(&responses[1].body).as_deref(),
        Some("script_error")
    );
    assert_eq!(responses[2].status, 200, "body: {}", responses[2].body);
    assert_eq!(responses[2].body, "abc");
    assert_eq!(responses[3].status, 400, "body: {}", responses[3].body);
    assert_eq!(
        error_class(&responses[3].body).as_deref(),
        Some("invalid_multipart")
    );
    assert_eq!(responses[4].status, 500, "body: {}", responses[4].body);
    assert_eq!(
        error_class(&responses[4].body).as_deref(),
        Some("script_error")
    );
}

#[test]
fn non_multipart_requests_expose_an_empty_files_array() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.respond(200, { "Content-Type": "text/plain" }, String(ctx.request.files.length) + ":" + JSON.stringify(ctx.request.files));
"#;
    let (response, _) = serve_upload_fixture(
        dir.path(),
        &upload_config(1024 * 1024),
        script,
        &[],
        |port| {
            request_with_body(
                port,
                "POST",
                "/demo/documents/manifest/group-a",
                &[("Content-Type", "text/plain")],
                "plain body",
            )
        },
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "0:[]");
}

#[test]
fn ctx_file_read_text_reads_a_file_inside_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files/data")).expect("files dir");
    std::fs::write(dir.path().join("files/data/hello.txt"), "hello file").expect("fixture file");
    let script = r#"
var text = ctx.file.readText("data/hello.txt");
ctx.respond(200, { "Content-Type": "text/plain; charset=utf-8" }, text);
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "hello file");
}

#[test]
fn ctx_file_errors_are_catchable_with_stable_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/latin1.bin"), [0x66, 0x6f, 0x80]).expect("fixture");
    std::fs::create_dir_all(dir.path().join("files/adir")).expect("fixture dir");
    let script = r#"
function code(fn) {
  try { fn(); return "no error"; } catch (error) { return error.code; }
}
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify({
  missing: code(function () { ctx.file.readText("nope.txt"); }),
  traversal: code(function () { ctx.file.readText("../outside.txt"); }),
  absolute: code(function () { ctx.file.readText("/etc/passwd"); }),
  encoding: code(function () { ctx.file.readText("latin1.bin"); }),
  directory: code(function () { ctx.file.readText("adir"); })
}));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["missing"], "file_not_found");
    assert_eq!(json["traversal"], "file_path_invalid");
    assert_eq!(json["absolute"], "file_path_invalid");
    assert_eq!(json["encoding"], "file_encoding_error");
    assert_eq!(json["directory"], "file_io_error");
}

#[cfg(unix)]
#[test]
fn ctx_file_allows_in_root_symlinks_and_rejects_symlink_escapes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside dir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/inside.txt"), "inside").expect("inside file");
    std::fs::write(outside.path().join("secret.txt"), "secret").expect("outside file");
    std::os::unix::fs::symlink("inside.txt", dir.path().join("files/inside-link.txt"))
        .expect("inside symlink");
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        dir.path().join("files/escape-link.txt"),
    )
    .expect("escape symlink");
    let script = r#"
var out = {};
try { out.allowed = ctx.file.readText("inside-link.txt"); } catch (error) { out.allowed = error.code; }
try { out.escaped = ctx.file.readText("escape-link.txt"); } catch (error) { out.escaped = error.code; }
ctx.respond(200, {}, JSON.stringify(out));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["allowed"], "inside");
    assert_eq!(json["escaped"], "file_path_invalid");
}

#[test]
fn ctx_file_reads_cap_at_eight_mib() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cap = 8 * 1024 * 1024;
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/at-cap.bin"), vec![b'a'; cap]).expect("at cap file");
    std::fs::write(dir.path().join("files/over-cap.bin"), vec![b'a'; cap + 1]).expect("over file");
    let script = r#"
var out = {};
out.atCap = ctx.file.readText("at-cap.bin").length;
try { ctx.file.readText("over-cap.bin"); out.overCapText = "no error"; }
catch (error) { out.overCapText = error.code; }
try { ctx.file.readBytes("over-cap.bin"); out.overCapBytes = "no error"; }
catch (error) { out.overCapBytes = error.code; }
ctx.respond(200, {}, JSON.stringify(out));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["atCap"], cap);
    assert_eq!(json["overCapText"], "file_too_large");
    assert_eq!(json["overCapBytes"], "file_too_large");
}

#[test]
fn ctx_file_read_bytes_returns_exact_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(
        dir.path().join("files/binary.bin"),
        [0_u8, 1, 127, 128, 255],
    )
    .expect("binary file");
    let script = r#"
var bytes = ctx.file.readBytes("binary.bin");
ctx.respond(200, {}, JSON.stringify({
  kind: bytes.constructor.name,
  values: Array.prototype.slice.call(bytes)
}));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["kind"], "Uint8Array");
    assert_eq!(json["values"], serde_json::json!([0, 1, 127, 128, 255]));
}

#[test]
fn ctx_file_stream_serves_the_full_body_as_an_opaque_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(
        dir.path().join("files/binary.bin"),
        [0_u8, 1, 127, 128, 255],
    )
    .expect("binary file");
    let script = r#"
var handle = ctx.file.stream("binary.bin");
var shape = {
  keys: Object.keys(handle).length,
  symbols: Object.getOwnPropertySymbols(handle).length,
  proto_null: Object.getPrototypeOf(handle) === null
};
if (shape.keys !== 0 || shape.symbols !== 0 || !shape.proto_null) {
  ctx.respond(500, {}, JSON.stringify(shape));
} else {
  ctx.respond(200, { "Content-Type": "application/octet-stream" }, handle);
}
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.header("content-length"), Some("5"));
    assert_eq!(response.header("accept-ranges"), Some("bytes"));
    assert_eq!(
        response.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(response.body_bytes, [0, 1, 127, 128, 255]);
}

#[test]
fn ctx_file_calls_are_logged_ordered_and_without_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/report.txt"), "four").expect("fixture file");
    let script = r#"
ctx.file.readText("report.txt");
ctx.respond(200, { "Content-Type": "text/plain" }, ctx.file.stream("report.txt"));
"#;
    let (response, stderr) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "four");
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    let calls = log["file_calls"].as_array().expect("file_calls array");
    assert_eq!(calls.len(), 2, "file_calls: {calls:?}");
    assert_eq!(calls[0]["api"], "file.readText");
    assert_eq!(calls[0]["bytes"], 4);
    assert_eq!(calls[0]["error"], serde_json::Value::Null);
    assert_eq!(calls[1]["api"], "file.stream");
    assert_eq!(calls[1]["bytes"], 4);
    assert_eq!(calls[1]["error"], serde_json::Value::Null);
    assert!(!stderr.contains("report.txt"), "stderr: {stderr}");
    assert!(
        !stderr.contains(&dir.path().display().to_string()),
        "stderr: {stderr}"
    );
}

#[test]
fn ctx_file_stream_answers_single_ranges_with_206() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/digits.txt"), "0123456789").expect("fixture file");
    let script = r#"
ctx.respond(200, {}, ctx.file.stream("digits.txt"));
"#;
    let (responses, _) = serve_fixture_dir(dir.path(), script, |port| {
        [
            request(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[("Range", "bytes=0-3")],
            ),
            request(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[("Range", "bytes=4-")],
            ),
            request(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[("Range", "bytes=-3")],
            ),
            request(
                port,
                "GET",
                "/demo/documents/manifest/group-a",
                &[("Range", "bytes=8-99")],
            ),
        ]
    });
    let expected = [
        ("bytes 0-3/10", "0123"),
        ("bytes 4-9/10", "456789"),
        ("bytes 7-9/10", "789"),
        ("bytes 8-9/10", "89"),
    ];
    for (response, (content_range, body)) in responses.iter().zip(expected) {
        assert_eq!(response.status, 206, "body: {}", response.body);
        let len = body.len().to_string();
        assert_eq!(response.header("content-range"), Some(content_range));
        assert_eq!(response.header("content-length"), Some(len.as_str()));
        assert_eq!(response.header("accept-ranges"), Some("bytes"));
        assert_eq!(response.body, body);
    }
}

#[test]
fn ctx_file_stream_answers_416_for_unusable_ranges() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/digits.txt"), "0123456789").expect("fixture file");
    std::fs::write(dir.path().join("files/empty.txt"), "").expect("empty file");
    let script = r"
ctx.respond(200, {}, ctx.file.stream(ctx.request.query.file));
";
    let range_request = |port: u16, file: &str, range: &str| {
        let path = format!("/demo/documents/manifest/group-a?file={file}");
        request(port, "GET", &path, &[("Range", range)])
    };
    let (responses, stderr) = serve_fixture_dir(dir.path(), script, |port| {
        let digits = "/demo/documents/manifest/group-a?file=digits.txt";
        [
            range_request(port, "digits.txt", "bytes=99-"),
            range_request(port, "digits.txt", "bytes=x-y"),
            range_request(port, "digits.txt", "bytes=0-1,3-4"),
            range_request(port, "empty.txt", "bytes=0-1"),
            request(
                port,
                "GET",
                digits,
                &[("Range", "bytes=0-1"), ("Range", "bytes=3-4")],
            ),
            request_raw_range(port, digits, b"bytes=\x80"),
            range_request(port, "digits.txt", "bytes=0 - 3"),
        ]
    });
    let expected = [
        "bytes */10",
        "bytes */10",
        "bytes */10",
        "bytes */0",
        "bytes */10",
        "bytes */10",
        "bytes */10",
    ];
    for (response, content_range) in responses.iter().zip(expected) {
        assert_eq!(response.status, 416, "body: {}", response.body);
        assert_eq!(response.header("content-range"), Some(content_range));
        assert_eq!(response.header("content-length"), Some("0"));
        assert!(
            response.body_bytes.is_empty(),
            "body: {:?}",
            response.body_bytes
        );
    }
    let logs: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|line| line.contains("\"request_id\""))
        .map(|line| serde_json::from_str(line).expect("structured log json"))
        .collect();
    assert_eq!(logs.len(), 7, "stderr: {stderr}");
    for log in logs {
        assert_eq!(log["file_calls"][0]["api"], "file.stream");
        assert_eq!(log["file_calls"][0]["bytes"], 0, "log: {log}");
        assert_eq!(log["file_calls"][0]["error"], serde_json::Value::Null);
    }
}

#[test]
fn ctx_file_stream_ignores_range_when_if_range_is_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/digits.txt"), "0123456789").expect("fixture file");
    let script = r#"
ctx.respond(200, {}, ctx.file.stream("digits.txt"));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(
            port,
            "GET",
            "/demo/documents/manifest/group-a",
            &[("Range", "bytes=0-3"), ("If-Range", "W/\"anything\"")],
        )
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.header("content-range"), None);
    assert_eq!(response.header("content-length"), Some("10"));
    assert_eq!(response.body, "0123456789");
}

#[test]
fn ctx_respond_rejects_framing_headers_and_non_200_status_for_file_streams() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/digits.txt"), "0123456789").expect("fixture file");
    let script = r#"
function attempt(status, headers) {
  try {
    ctx.respond(status, headers, ctx.file.stream("digits.txt"));
    return "no error";
  } catch (error) {
    return error.code;
  }
}
var out = {
  contentLength: attempt(200, { "Content-Length": "4" }),
  contentRange: attempt(200, { "Content-Range": "bytes 0-1/10" }),
  acceptRanges: attempt(200, { "Accept-Ranges": "none" }),
  non200: attempt(201, {})
};
ctx.respond(200, { "Content-Type": "application/json" }, JSON.stringify(out));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    assert_eq!(json["contentLength"], "script_error");
    assert_eq!(json["contentRange"], "script_error");
    assert_eq!(json["acceptRanges"], "script_error");
    assert_eq!(json["non200"], "script_error");
}

#[test]
fn ctx_file_stream_client_disconnect_mid_body_is_logged() {
    // Large enough that the relay cannot finish before the client aborts: the
    // harness reads one body byte and closes the socket.
    let size = 8 * 1024 * 1024;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/big.bin"), vec![b'x'; size]).expect("big file");
    let script = r#"
ctx.respond(200, {}, ctx.file.stream("big.bin"));
"#;
    let ((), stderr) = serve_fixture_dir(dir.path(), script, |port| {
        abort_after_response_head(port, "/demo/documents/manifest/group-a");
    });
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["status"], 200);
    assert_eq!(log["error"], "client_disconnected", "log: {line}");
    let call = &log["file_calls"][0];
    assert_eq!(call["api"], "file.stream");
    assert_eq!(call["error"], "client_disconnected");
    let relayed = call["bytes"].as_u64().expect("relayed byte count");
    assert!(
        relayed < u64::try_from(size).expect("body length"),
        "the relay finished before the client aborted: {line}"
    );
}

#[test]
fn ctx_file_stream_truncation_after_headers_is_logged_as_file_stream_error() {
    let size = 16 * 1024 * 1024;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    let file_path = dir.path().join("files/big.bin");
    std::fs::write(&file_path, vec![b'x'; size]).expect("big file");
    let script = r#"
ctx.respond(200, {}, ctx.file.stream("big.bin"));
"#;
    let (received, stderr) = serve_fixture_dir(dir.path(), script, |port| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        stream
            .write_all(
                b"GET /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .expect("write request");
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("read response head");
            assert_ne!(read, 0, "server closed before the response head");
            received.extend_from_slice(&buffer[..read]);
            if received.windows(4).any(|window| window == b"\r\n\r\n") {
                // Let the relay fill its bounded channel and the socket buffers
                // while this client stalls, then shrink the backing file: the
                // announced Content-Length can no longer be satisfied.
                std::thread::sleep(Duration::from_millis(300));
                std::fs::File::create(&file_path).expect("truncate file");
                let _ = stream.read_to_end(&mut received);
                break;
            }
        }
        received
    });
    let line = stderr
        .lines()
        .find(|line| line.contains("\"request_id\""))
        .unwrap_or_else(|| panic!("request log missing: {stderr}"));
    let log: serde_json::Value = serde_json::from_str(line).expect("structured log json");
    assert_eq!(log["status"], 200);
    assert_eq!(log["error"], "file_stream_error", "log: {line}");
    assert_eq!(log["file_calls"][0]["api"], "file.stream");
    assert_eq!(log["file_calls"][0]["error"], "file_stream_error");
    let relayed = log["file_calls"][0]["bytes"]
        .as_u64()
        .expect("relayed byte count");
    assert!(
        relayed < u64::try_from(size).expect("body length"),
        "the relay delivered the whole file before truncation: {line}"
    );
    assert!(
        received.len() < size,
        "client received {} bytes of an announced {size}",
        received.len()
    );
}

#[test]
fn uncaught_file_error_maps_to_script_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
ctx.file.readText("nope.txt");
ctx.respond(200, {}, "unreachable");
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 500, "body: {}", response.body);
    assert_eq!(error_class(&response.body).as_deref(), Some("script_error"));
}

#[test]
fn ctx_file_root_removed_at_runtime_is_catchable() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    std::fs::write(dir.path().join("files/hello.txt"), "hello").expect("fixture file");
    let script = r#"
var code = "read unexpectedly succeeded";
try { ctx.file.readText("hello.txt"); } catch (error) { code = error.code; }
ctx.respond(200, {}, code);
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        std::fs::remove_dir_all(dir.path().join("files")).expect("remove files root");
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "file_io_error");
}

#[cfg(unix)]
#[test]
fn ctx_file_rejects_non_regular_files_without_blocking() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    let fifo = dir.path().join("files/pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed with {status}");
    let script = r#"
var code = "read unexpectedly succeeded";
try { ctx.file.readText("pipe"); } catch (error) { code = error.code; }
ctx.respond(200, {}, code);
"#;
    // Short deadline: without the pre-open check the FIFO open blocks until
    // the script timeout, so the assertion fails fast instead of hanging.
    let config = with_sandbox(good_config(), 500);
    let (response, _) = serve_and_run(
        |port| fixture_with_script(dir.path(), port, &config, script),
        &[],
        &[],
        |port| request(port, "GET", "/demo/documents/manifest/group-a", &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "file_io_error");
}

#[test]
fn ctx_file_never_resolves_url_style_or_platform_specific_names_outside_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("files")).expect("files dir");
    let script = r#"
function code(path) {
  try { ctx.file.readText(path); return "read unexpectedly succeeded"; } catch (error) { return error.code; }
}
ctx.respond(200, {}, JSON.stringify({
  url: code("file:///etc/passwd"),
  drive: code("C:\\Windows\\win.ini"),
  backslash: code("..\\..\\secret")
}));
"#;
    let (response, _) = serve_fixture_dir(dir.path(), script, |port| {
        request(port, "GET", "/demo/documents/manifest/group-a", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
    // URL-shaped strings stay relative names inside the root, never system paths.
    assert_eq!(json["url"], "file_not_found");
    #[cfg(unix)]
    {
        // Backslashes are ordinary name bytes on Unix, so these stay in-root.
        assert_eq!(json["drive"], "file_not_found");
        assert_eq!(json["backslash"], "file_not_found");
    }
    #[cfg(windows)]
    {
        // On Windows the same strings are absolute and parent-directory escapes.
        assert_eq!(json["drive"], "file_path_invalid");
        assert_eq!(json["backslash"], "file_path_invalid");
    }
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
        |port| abort_after_response_head(port, "/demo/documents/manifest/group-a"),
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
    for file in ["metadata.json", "DOC-0001.pdf", "DOC-0002.pdf"] {
        std::fs::copy(
            source.join("files").join(file),
            dir.join("files").join(file),
        )
        .expect("copy demo file");
    }
    for script in [
        "manifest.js",
        "download.js",
        "local-manifest.js",
        "local-download.js",
        "upload.js",
    ] {
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
    with_demo_prepared(env, |_| {}, |port, _| run_tests(port))
}

/// Serve a copy of the repository demo fixture with `env` set. `prepare` runs
/// after the copy and before the server starts, so a test can substitute
/// fixture data; `run_tests` also receives the fixture directory so it can
/// assert what the routes wrote — and did not write — to disk.
fn with_demo_prepared<T>(
    env: &[(&str, &str)],
    prepare: impl Fn(&Path),
    run_tests: impl FnOnce(u16, &Path) -> T,
) -> (T, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    serve_and_run(
        |port| {
            let config = demo_fixture(&path, port);
            prepare(&path);
            config
        },
        env,
        &[],
        |port| run_tests(port, &path),
    )
}

fn demo_manifest_request(port: u16, headers: &[(&str, &str)]) -> Response {
    request(port, "GET", "/demo/documents/manifest/group-a", headers)
}

fn demo_local_manifest_request(port: u16, headers: &[(&str, &str)]) -> Response {
    request(
        port,
        "GET",
        "/demo/documents/local-manifest/group-a",
        headers,
    )
}

fn demo_local_download_request(port: u16, document_id: &str, headers: &[(&str, &str)]) -> Response {
    request(
        port,
        "GET",
        &format!("/demo/documents/local-download/{document_id}"),
        headers,
    )
}

/// The committed bytes of one demo PDF fixture.
fn fixture_pdf(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(DEMO_DIR).join("files").join(name)).expect("read demo pdf")
}

/// Sorted file names in a directory, used to assert that a route wrote nothing.
fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
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

// Offline file-slice routes: the same document domain served from
// demo/files/ without an upstream or an environment override.

#[test]
fn demo_local_manifest_route_returns_the_catalog_csv() {
    let (response, _) = with_demo(&[], |port| demo_local_manifest_request(port, &[]));
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
fn demo_local_manifest_route_escapes_csv_fields() {
    // Same escaping case the upstream manifest route covers; the offline
    // route must answer the identical CSV contract.
    let metadata = r#"{"data":[
{"code":"DOC-0003","title":"示例,设备 \"A\"\n第二行","system_code":"SYS,C","file":"DOC-0002.pdf"},
{"code":"DOC-0004","title":"回车\r换行","system_code":"SYS-D","file":"DOC-0002.pdf"}
]}"#;
    let (response, _) = with_demo_prepared(
        &[],
        |dir| std::fs::write(dir.join("files/metadata.json"), metadata).expect("write metadata"),
        |port, _| demo_local_manifest_request(port, &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        "文件编码,文件标题,系统代码\n\
         DOC-0003,\"示例,设备 \"\"A\"\"\n第二行\",\"SYS,C\"\n\
         DOC-0004,\"回车\r换行\",SYS-D\n"
    );
}

#[test]
fn demo_local_manifest_route_returns_header_only_for_empty_data() {
    let (response, _) = with_demo_prepared(
        &[],
        |dir| {
            std::fs::write(dir.join("files/metadata.json"), br#"{"data":[]}"#)
                .expect("write metadata");
        },
        |port, _| demo_local_manifest_request(port, &[]),
    );
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body, "文件编码,文件标题,系统代码\n");
}

#[test]
fn demo_local_download_route_streams_the_fixture_pdf() {
    let pdf = fixture_pdf("DOC-0001.pdf");
    let (response, _) = with_demo(&[], |port| {
        demo_local_download_request(port, "DOC-0001", &[])
    });
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.header("content-type"), Some("application/pdf"));
    assert_eq!(
        response.header("content-disposition"),
        Some("attachment;filename=\"DOC-0001.pdf\"")
    );
    assert_eq!(response.header("accept-ranges"), Some("bytes"));
    assert_eq!(
        response.header("content-length"),
        Some(pdf.len().to_string().as_str())
    );
    assert_eq!(response.body_bytes, pdf);
}

#[test]
fn demo_local_download_route_serves_a_range_request() {
    let pdf = fixture_pdf("DOC-0001.pdf");
    let (response, _) = with_demo(&[], |port| {
        demo_local_download_request(port, "DOC-0001", &[("Range", "bytes=0-4")])
    });
    assert_eq!(response.status, 206, "body: {}", response.body);
    assert_eq!(
        response.header("content-range"),
        Some(format!("bytes 0-4/{}", pdf.len()).as_str())
    );
    assert_eq!(response.header("content-length"), Some("5"));
    assert_eq!(response.body_bytes, pdf[..5]);
}

#[test]
fn demo_local_download_route_answers_416_for_an_unusable_range() {
    let pdf = fixture_pdf("DOC-0001.pdf");
    let ((unsatisfiable, malformed), _) = with_demo(&[], |port| {
        (
            demo_local_download_request(port, "DOC-0001", &[("Range", "bytes=999999-")]),
            demo_local_download_request(port, "DOC-0001", &[("Range", "bytes=abc")]),
        )
    });
    for response in [&unsatisfiable, &malformed] {
        assert_eq!(response.status, 416, "body: {}", response.body);
        assert_eq!(
            response.header("content-range"),
            Some(format!("bytes */{}", pdf.len()).as_str())
        );
        assert!(
            response.body_bytes.is_empty(),
            "416 carries no body: {}",
            response.body
        );
    }
}

#[test]
fn demo_local_download_route_answers_404_for_an_unknown_document() {
    let (response, _) = with_demo(&[], |port| {
        demo_local_download_request(port, "DOC-9999", &[])
    });
    assert_eq!(response.status, 404, "body: {}", response.body);
    assert_eq!(
        response.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("document_not_found"),
        "body: {}",
        response.body
    );
}

#[test]
fn demo_upload_route_reports_the_file_and_keeps_no_state() {
    const UPLOAD: &[u8] = b"%PDF-1.4\n% uploaded, never stored\n%%EOF\n";
    let ((), _) = with_demo_prepared(
        &[],
        |_| {},
        |port, dir| {
            let files_dir = dir.join("files");
            let before = dir_listing(&files_dir);
            let response = request_multipart(
                port,
                "/demo/documents/upload",
                "sd-demo-upload",
                &[
                    MultipartPart::field("note", b"ignored"),
                    MultipartPart::file("document", "DOC-0002.pdf", "application/pdf", UPLOAD),
                ],
            );
            assert_eq!(response.status, 201, "body: {}", response.body);
            assert_eq!(
                response.header("content-type"),
                Some("application/json; charset=utf-8")
            );
            let json: serde_json::Value = serde_json::from_str(&response.body).expect("json body");
            assert_eq!(json["field"], "document");
            assert_eq!(json["filename"], "DOC-0002.pdf");
            assert_eq!(json["content_type"], "application/pdf");
            assert_eq!(json["size"], serde_json::json!(UPLOAD.len()));

            // A later request cannot observe the upload: the committed fixture
            // still serves its own bytes, and the upload wrote nothing to the
            // static file root.
            let download = demo_local_download_request(port, "DOC-0002", &[]);
            assert_eq!(download.status, 200, "body: {}", download.body);
            assert_eq!(download.body_bytes, fixture_pdf("DOC-0002.pdf"));
            assert_eq!(dir_listing(&files_dir), before);
        },
    );
}

/// A matched route whose body never finishes must be cut off by the configured
/// request deadline instead of holding the connection until the client leaves.
#[test]
fn slow_non_multipart_body_answers_408_request_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = with_partial_body_route(good_config(), 300);
    let (response, _) = serve_upload_fixture(dir.path(), &config, OK_SCRIPT, &[], |port| {
        request_partial_body(
            port,
            "/demo/documents/manifest/group-a",
            "text/plain",
            64 * 1024,
            b"partial body",
        )
    });
    assert_eq!(response.status, 408, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("request_timeout"),
        "body: {}",
        response.body
    );
}

/// The multipart path must time out mid-parse, answer 408, and drop the
/// request-scoped temporary storage the parse had already created.
#[test]
fn slow_multipart_body_answers_408_and_cleans_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let temp_root = upload_temp_root(dir.path());
    let config = with_partial_body_route(good_config(), 300);
    let partial = b"--sd-slow-body\r\nContent-Disposition: form-data; name=\"document\"; \
        filename=\"big.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nabc";
    let temp_env = temp_root.to_str().expect("temp root path");
    let (response, _) = serve_upload_fixture(
        dir.path(),
        &config,
        OK_SCRIPT,
        &[("TMPDIR", temp_env), ("TMP", temp_env), ("TEMP", temp_env)],
        |port| {
            request_partial_body(
                port,
                "/demo/documents/manifest/group-a",
                "multipart/form-data; boundary=sd-slow-body",
                64 * 1024,
                partial,
            )
        },
    );
    assert_eq!(response.status, 408, "body: {}", response.body);
    assert_eq!(
        error_class(&response.body).as_deref(),
        Some("request_timeout"),
        "body: {}",
        response.body
    );
    wait_for_empty_temp(&temp_root);
}

/// A client that never finishes its request head must be disconnected at the
/// configured deadline instead of holding a connection slot open.
#[test]
fn slow_request_head_is_closed_at_the_deadline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = with_request_timeout(good_config(), 300);
    let ((), log) = serve_upload_fixture(dir.path(), &config, OK_SCRIPT, &[], |port| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        stream
            .write_all(b"GET /demo/documents/manifest/group-a HTTP/1.1\r\nHost: 127.0.0.1\r\n")
            .expect("write partial head");
        stream.flush().expect("flush");
        // A half-read head has no HTTP answer: the server must close instead.
        let mut buffer = [0_u8; 64];
        let read = stream
            .read(&mut buffer)
            .expect("read until the server closes");
        assert_eq!(
            read,
            0,
            "expected a close, got: {:?}",
            String::from_utf8_lossy(&buffer[..read])
        );
    });
    assert!(
        log.contains("request_head_timeout"),
        "log missing the head timeout class: {log}"
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
