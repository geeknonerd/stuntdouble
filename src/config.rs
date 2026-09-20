#![allow(clippy::struct_field_names, clippy::single_match_else)]
// Configuration contract: docs/contracts/config.md
use std::fmt;
use std::path::{Path, PathBuf};

const HTTP_METHODS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

#[derive(Debug, Clone)]
pub struct Config {
    pub config_version: String,
    pub server: ServerConfig,
    pub files: FilesConfig,
    pub routes: Vec<Route>,
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

    let root: toml::Value = text
        .parse()
        .map_err(|e: toml::de::Error| ConfigError::Syntax {
            path: path.to_path_buf(),
            // toml_edit renders syntax errors with line and column.
            message: e.to_string(),
        })?;

    let Some(root_table) = root.as_table() else {
        return Err(schema_error(path, vec![bad("(root)", "table", &root)]));
    };

    let mut v = Vec::new();
    reject_unknown(
        root_table,
        &["config_version", "server", "files", "routes"],
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
        routes,
    })
}

fn schema_error(path: &Path, violations: Vec<Violation>) -> ConfigError {
    ConfigError::Schema {
        path: path.to_path_buf(),
        violations,
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
