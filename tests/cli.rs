//! End-to-end checks through the only seam: the built binary plus real HTTP.
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    let mut child = serve_with_env(&config, env);
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
