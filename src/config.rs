// Configuration contract: docs/contracts/config.md
use std::fmt;
use std::path::{Path, PathBuf};

const HTTP_METHODS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

#[derive(Debug, Clone)]
// Field mirrors the public TOML key `config_version`; renaming it would break
// the configuration contract, so the pedantic lint is suppressed here only.
#[allow(clippy::struct_field_names)]
pub struct Config {
    pub config_version: String,
    pub server: ServerConfig,
    pub files: FilesConfig,
    pub sandbox: SandboxConfig,
    pub upstream: UpstreamConfig,
    pub routes: Vec<Route>,
}

/// Script execution limits. The timeout is a wall-clock deadline enforced by
/// the host; see `script::execute` for the Boa 0.22 interruption ceiling.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub script_timeout_ms: u64,
}

/// Upstream HTTP access limits for `ctx.http.get`.
#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    pub allow_hosts: Vec<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct FilesConfig {
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub name: Option<String>,
    pub method: String,
    pub path: String,
    pub script: PathBuf,
}

#[derive(Debug)]
pub struct Violation {
    pub field: String,
    pub expected: String,
    pub actual: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: expected {}, got {}",
            self.field, self.expected, self.actual
        )
    }
}

#[derive(Debug)]
pub enum ConfigError {
    /// Could not read the configuration file
    Read { path: PathBuf, reason: String },
    /// TOML syntax error, message contains line/column
    Syntax { path: PathBuf, message: String },
    /// Invalid configuration values
    Schema {
        path: PathBuf,
        violations: Vec<Violation>,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, reason } => {
                write!(
                    f,
                    "{}: cannot read configuration: {}",
                    path.display(),
                    reason
                )
            }
            ConfigError::Syntax { path, message } => {
                write!(f, "{}: invalid TOML: {}", path.display(), message)
            }
            ConfigError::Schema { path, violations } => {
                writeln!(f, "{}: invalid configuration", path.display())?;
                for v in violations {
                    writeln!(f, "  {v}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

type Table = toml::map::Map<String, toml::Value>;

/// Load and validate a TOML configuration file. Every field problem is
/// reported at once so a single run surfaces the whole list.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    // NOTE: `toml::from_str` parses a document; `str::parse::<toml::Value>`
    // parses a single inline value as of toml 1.x and rejects whole documents.
    let root: toml::Value = toml::from_str(&text).map_err(|e: toml::de::Error| {
        ConfigError::Syntax {
            path: path.to_path_buf(),
            // the parser renders syntax errors with line and column.
            message: e.to_string(),
        }
    })?;

    let Some(root_table) = root.as_table() else {
        return Err(schema_error(path, vec![bad("(root)", "table", &root)]));
    };

    let mut v = Vec::new();
    reject_unknown(
        root_table,
        &[
            "config_version",
            "server",
            "files",
            "sandbox",
            "upstream",
            "routes",
        ],
        "",
        &mut v,
    );

    let config_version =
        req_string(root_table, "config_version", "config_version", &mut v).unwrap_or_default();

    let server = match root_table.get("server") {
        None => {
            v.push(missing("server", "table"));
            ServerConfig {
                bind: default_bind(),
                port: default_port(),
            }
        }
        Some(toml::Value::Table(t)) => {
            reject_unknown(t, &["bind", "port"], "server", &mut v);
            ServerConfig {
                bind: opt_string(t, "bind", "server.bind", &mut v).unwrap_or_else(default_bind),
                port: opt_port(t, "server.port", &mut v).unwrap_or_else(default_port),
            }
        }
        Some(other) => {
            v.push(bad("server", "table", other));
            ServerConfig {
                bind: default_bind(),
                port: default_port(),
            }
        }
    };

    let root_dir = match root_table.get("files") {
        None => {
            v.push(missing("files", "table"));
            None
        }
        Some(toml::Value::Table(t)) => {
            reject_unknown(t, &["root"], "files", &mut v);
            req_string(t, "root", "files.root", &mut v).map(|raw| {
                let p = PathBuf::from(&raw);
                let base = path.parent().unwrap_or(Path::new("."));
                if p.is_absolute() {
                    p
                } else {
                    base.join(p)
                }
            })
        }
        Some(other) => {
            v.push(bad("files", "table", other));
            None
        }
    };

    // Optional: an absent [sandbox] keeps every default.
    let sandbox = parse_sandbox(root_table, &mut v);

    // Optional: an absent [upstream] denies every host by default.
    let upstream = parse_upstream(root_table, &mut v);

    let routes = parse_routes(root_table, path, &mut v);

    if !v.is_empty() {
        return Err(schema_error(path, v));
    }

    let root_dir = root_dir.expect("files.root presence is enforced by violations");
    if !root_dir.is_dir() {
        return Err(schema_error(
            path,
            vec![Violation {
                field: "files.root".into(),
                expected: "existing directory".into(),
                actual: root_dir.display().to_string(),
            }],
        ));
    }

    Ok(Config {
        config_version,
        server,
        files: FilesConfig { root: root_dir },
        sandbox,
        upstream,
        routes,
    })
}

fn schema_error(path: &Path, violations: Vec<Violation>) -> ConfigError {
    ConfigError::Schema {
        path: path.to_path_buf(),
        violations,
    }
}

/// Parse the optional `[sandbox]` table; absent fields keep their defaults.
fn parse_sandbox(root: &Table, v: &mut Vec<Violation>) -> SandboxConfig {
    let defaults = SandboxConfig {
        script_timeout_ms: default_script_timeout_ms(),
    };
    match root.get("sandbox") {
        None => defaults,
        Some(toml::Value::Table(t)) => {
            reject_unknown(t, &["script_timeout_ms"], "sandbox", v);
            SandboxConfig {
                script_timeout_ms: opt_duration_ms(
                    t,
                    "script_timeout_ms",
                    "sandbox.script_timeout_ms",
                    v,
                )
                .unwrap_or_else(default_script_timeout_ms),
            }
        }
        Some(other) => {
            v.push(bad("sandbox", "table", other));
            defaults
        }
    }
}

/// Normalize one allowlist entry to the shape `url::Url::host()` returns:
/// a lowercase/punycoded domain, or an unbracketed IP literal. Entries use URL
/// host syntax, so IPv6 literals are bracketed (`"[::1]"`).
fn normalize_host(raw: &str) -> Option<String> {
    match url::Host::parse(raw) {
        Ok(url::Host::Domain(domain)) => Some(domain),
        Ok(url::Host::Ipv4(ip)) => Some(ip.to_string()),
        Ok(url::Host::Ipv6(ip)) => Some(ip.to_string()),
        Err(_) => None,
    }
}

/// Parse the optional `[upstream]` table; absent fields keep safe defaults.
fn parse_upstream(root: &Table, v: &mut Vec<Violation>) -> UpstreamConfig {
    let defaults = UpstreamConfig {
        allow_hosts: Vec::new(),
        timeout_ms: default_upstream_timeout_ms(),
    };
    match root.get("upstream") {
        None => defaults,
        Some(toml::Value::Table(t)) => {
            reject_unknown(t, &["allow_hosts", "timeout_ms"], "upstream", v);
            UpstreamConfig {
                allow_hosts: opt_host_list(t, "upstream.allow_hosts", v).unwrap_or_default(),
                timeout_ms: opt_duration_ms(t, "timeout_ms", "upstream.timeout_ms", v)
                    .unwrap_or_else(default_upstream_timeout_ms),
            }
        }
        Some(other) => {
            v.push(bad("upstream", "table", other));
            defaults
        }
    }
}

/// Array of non-empty host names. An omitted allowlist denies every host.
fn opt_host_list(table: &Table, field: &str, out: &mut Vec<Violation>) -> Option<Vec<String>> {
    match table.get("allow_hosts") {
        None => None,
        Some(toml::Value::Array(items)) => {
            let mut hosts = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                match item {
                    toml::Value::String(host) if !host.is_empty() => {
                        if let Some(host) = normalize_host(host) {
                            hosts.push(host);
                        } else {
                            out.push(Violation {
                                field: format!("{field}[{index}]"),
                                expected: "host name or IP literal without scheme or port".into(),
                                actual: format!("string {host:?}"),
                            });
                        }
                    }
                    other => out.push(Violation {
                        field: format!("{field}[{index}]"),
                        expected: "non-empty host string".into(),
                        actual: describe(other),
                    }),
                }
            }
            Some(hosts)
        }
        Some(other) => {
            out.push(bad(field, "array of host strings", other));
            None
        }
    }
}

fn parse_routes(root: &Table, config_path: &Path, v: &mut Vec<Violation>) -> Vec<Route> {
    let Some(value) = root.get("routes") else {
        v.push(missing("routes", "array of route tables"));
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        v.push(bad("routes", "array of route tables", value));
        return Vec::new();
    };

    let base = config_path.parent().unwrap_or(Path::new("."));
    let mut routes = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let prefix = format!("routes[{idx}]");
        let Some(table) = item.as_table() else {
            v.push(bad(&prefix, "table", item));
            continue;
        };
        reject_unknown(table, &["name", "method", "path", "script"], &prefix, v);

        let name = opt_string(table, "name", &format!("{prefix}.name"), v);
        let method =
            req_string(table, "method", &format!("{prefix}.method"), v).unwrap_or_default();
        let path = req_string(table, "path", &format!("{prefix}.path"), v).unwrap_or_default();
        let script =
            req_string(table, "script", &format!("{prefix}.script"), v).unwrap_or_default();

        let method = if let Some(m) = normalize_method(&method) {
            m
        } else {
            v.push(Violation {
                field: format!("{prefix}.method"),
                expected: format!("one of {}", HTTP_METHODS.join(", ")),
                actual: method.clone(),
            });
            method.to_ascii_uppercase()
        };

        if !path.starts_with('/') {
            v.push(Violation {
                field: format!("{prefix}.path"),
                expected: "absolute path starting with /".into(),
                actual: path.clone(),
            });
        }
        validate_script_extension(&script, &prefix, v);

        let script_path = if PathBuf::from(&script).is_absolute() {
            PathBuf::from(&script)
        } else {
            base.join(&script)
        };

        routes.push(Route {
            name,
            method,
            path,
            script: script_path,
        });
    }
    routes
}

fn validate_script_extension(script: &str, prefix: &str, v: &mut Vec<Violation>) {
    let field = format!("{prefix}.script");
    let Some(ext) = Path::new(script).extension().and_then(|e| e.to_str()) else {
        v.push(Violation {
            field,
            expected: "file name ending in .js".into(),
            actual: script.into(),
        });
        return;
    };
    match ext {
        // Python lands in a later slice; reject early with the real reason.
        "py" => v.push(Violation {
            field,
            expected: "JavaScript file (the Python runtime is not supported in this slice)".into(),
            actual: script.into(),
        }),
        "js" | "mjs" | "cjs" => {}
        other => v.push(Violation {
            field,
            expected: format!("file name ending in .js, not .{other}"),
            actual: script.into(),
        }),
    }
}

/// Must use result to avoid silent failures.
#[must_use]
pub fn normalize_method(method: &str) -> Option<String> {
    let upper = method.to_ascii_uppercase();
    HTTP_METHODS.contains(&upper.as_str()).then_some(upper)
}

fn reject_unknown(table: &Table, allowed: &[&str], prefix: &str, out: &mut Vec<Violation>) {
    for key in table.keys() {
        if allowed.contains(&key.as_str()) {
            continue;
        }
        let field = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        out.push(Violation {
            field,
            expected: format!("one of [{}]", allowed.join(", ")),
            actual: "unknown key".into(),
        });
    }
}

/// Required string: an absent key is reported.
fn req_string(table: &Table, key: &str, field: &str, out: &mut Vec<Violation>) -> Option<String> {
    if !table.contains_key(key) {
        out.push(missing(field, "string"));
        return None;
    }
    opt_string(table, key, field, out)
}

/// Optional string: an absent key stays quiet and yields the caller default.
fn opt_string(table: &Table, key: &str, field: &str, out: &mut Vec<Violation>) -> Option<String> {
    match table.get(key) {
        None => None,
        Some(toml::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(toml::Value::String(s)) => {
            out.push(Violation {
                field: field.into(),
                expected: "non-empty string".into(),
                actual: s.clone(),
            });
            None
        }
        Some(other) => {
            out.push(bad(field, "string", other));
            None
        }
    }
}

fn opt_port(table: &Table, field: &str, out: &mut Vec<Violation>) -> Option<u16> {
    match table.get("port") {
        None => None,
        Some(toml::Value::Integer(n)) => match u16::try_from(*n) {
            Ok(0) | Err(_) => {
                out.push(Violation {
                    field: field.into(),
                    expected: "integer in [1, 65535]".into(),
                    actual: n.to_string(),
                });
                None
            }
            Ok(p) => Some(p),
        },
        Some(other) => {
            out.push(bad(field, "integer in [1, 65535]", other));
            None
        }
    }
}

/// Millisecond duration: a positive integer, because 0 would make every
/// request time out before the script can run.
fn opt_duration_ms(table: &Table, key: &str, field: &str, out: &mut Vec<Violation>) -> Option<u64> {
    match table.get(key) {
        None => None,
        Some(toml::Value::Integer(n)) => match u64::try_from(*n) {
            Ok(0) | Err(_) => {
                out.push(Violation {
                    field: field.into(),
                    expected: "integer number of milliseconds greater than 0".into(),
                    actual: n.to_string(),
                });
                None
            }
            Ok(ms) => Some(ms),
        },
        Some(other) => {
            out.push(bad(field, "integer number of milliseconds", other));
            None
        }
    }
}

fn missing(field: &str, expected: &str) -> Violation {
    Violation {
        field: field.into(),
        expected: expected.into(),
        actual: "missing".into(),
    }
}

fn bad(field: &str, expected: &str, value: &toml::Value) -> Violation {
    Violation {
        field: field.into(),
        expected: expected.into(),
        actual: describe(value),
    }
}

fn describe(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("string \"{s}\""),
        toml::Value::Integer(i) => format!("integer {i}"),
        toml::Value::Float(f) => format!("float {f}"),
        toml::Value::Boolean(b) => format!("boolean {b}"),
        toml::Value::Datetime(d) => format!("datetime {d}"),
        toml::Value::Array(a) => format!("array of {}", a.len()),
        toml::Value::Table(t) => {
            let keys: Vec<&String> = t.keys().collect();
            format!(
                "table with keys [{}]",
                keys.iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn default_bind() -> String {
    String::from("127.0.0.1")
}

fn default_port() -> u16 {
    3000
}

fn default_script_timeout_ms() -> u64 {
    10_000
}

fn default_upstream_timeout_ms() -> u64 {
    15_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir.join("files")).expect("files dir");
        let path = dir.join("stuntdouble.toml");
        std::fs::write(&path, body).expect("write config");
        path
    }

    fn minimal() -> &'static str {
        r#"
config_version = "1"

[server]

[files]
root = "./files"

[[routes]]
method = "GET"
path = "/x"
script = "scripts/x.js"
"#
    }

    #[test]
    fn upstream_allow_hosts_are_normalized_and_reject_non_hosts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let table = "[upstream]\nallow_hosts = [\"Example.COM\", \"[::1]\"]\n\n[files]";
        let configured = minimal().replace("[files]", table);
        let config = load(&write_config(dir.path(), &configured)).expect("load hosts");
        assert_eq!(
            config.upstream.allow_hosts,
            vec!["example.com".to_string(), "::1".to_string()]
        );

        let bad = minimal().replace(
            "[files]",
            "[upstream]\nallow_hosts = [\"example.com:8080\"]\n\n[files]",
        );
        let error = load(&write_config(dir.path(), &bad)).expect_err("port must fail");
        assert!(
            error.to_string().contains("upstream.allow_hosts[0]"),
            "{error}"
        );
    }

    #[test]
    fn upstream_defaults_deny_every_host_with_a_fifteen_second_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_config(dir.path(), minimal());
        let config = load(&path).expect("load");
        assert!(config.upstream.allow_hosts.is_empty());
        assert_eq!(config.upstream.timeout_ms, 15_000);
    }

    #[test]
    fn sandbox_timeout_defaults_to_ten_seconds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_config(dir.path(), minimal());
        let config = load(&path).expect("load");
        assert_eq!(config.sandbox.script_timeout_ms, 10_000);
    }

    #[test]
    fn sandbox_timeout_is_read_and_validated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let table = "[sandbox]\nscript_timeout_ms = {value}\n\n[files]";
        let configured = minimal().replace("[files]", &table.replace("{value}", "250"));
        let config = load(&write_config(dir.path(), &configured)).expect("load 250");
        assert_eq!(config.sandbox.script_timeout_ms, 250);

        let zero = minimal().replace("[files]", &table.replace("{value}", "0"));
        let error = load(&write_config(dir.path(), &zero)).expect_err("zero must fail");
        assert!(
            error.to_string().contains("sandbox.script_timeout_ms"),
            "{error}"
        );
    }
}
