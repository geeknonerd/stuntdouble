//! Rooted local file access for `ctx.file` plus request-scoped uploads.
//! Contracts: docs/contracts/ctx-api.md, docs/contracts/config.md
//!
//! Every script-visible path resolves against the configured static file root.
//! Absolute paths and `..` components are rejected before resolution, and the
//! canonicalized target must stay inside the canonicalized root, so a symlink
//! cannot widen the readable set. The remaining TOCTOU window between
//! resolution and open is an accepted tradeoff for this local, semi-trusted
//! model; `SECURITY.md` records it and the `openat`/`O_NOFOLLOW` upgrade
//! path.
use axum::extract::{FromRequest as _, Multipart, Request};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::HeaderMap;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt as _;

/// Buffered read cap shared by `ctx.file.readText` and `ctx.file.readBytes`.
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// One script-visible `ctx.file` operation.
#[derive(Debug, Clone, Copy)]
enum FileOp {
    ReadText,
    ReadBytes,
    Stream,
}

impl FileOp {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "readText" => Some(Self::ReadText),
            "readBytes" => Some(Self::ReadBytes),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }

    fn api(self) -> &'static str {
        match self {
            Self::ReadText => "file.readText",
            Self::ReadBytes => "file.readBytes",
            Self::Stream => "file.stream",
        }
    }
}

/// Why one `ctx.file` call failed. Codes are stable script-visible values.
#[derive(Debug)]
pub enum Error {
    PathInvalid(&'static str),
    NotFound,
    TooLarge,
    Encoding,
    Io(String),
}

impl Error {
    /// Stable class surfaced to the script as `error.code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathInvalid(_) => "file_path_invalid",
            Self::NotFound => "file_not_found",
            Self::TooLarge => "file_too_large",
            Self::Encoding => "file_encoding_error",
            Self::Io(_) => "file_io_error",
        }
    }

    /// Operator-facing reason; never carries a file path.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::PathInvalid(reason) => (*reason).to_string(),
            Self::NotFound => "file not found".to_string(),
            Self::TooLarge => format!("file exceeds the {MAX_READ_BYTES}-byte read cap"),
            Self::Encoding => "file is not valid UTF-8".to_string(),
            Self::Io(message) => message.clone(),
        }
    }
}

/// One `ctx.file` call, serialized into the per-request log line.
#[derive(Debug, Clone)]
pub struct CallRecord {
    api: &'static str,
    bytes: Option<u64>,
    duration_ms: Option<f64>,
    error: Option<&'static str>,
}

impl CallRecord {
    fn new(api: &'static str) -> Self {
        Self {
            api,
            bytes: None,
            duration_ms: None,
            error: None,
        }
    }

    /// Log shape shared by every file call. Paths never enter it.
    #[must_use]
    pub fn to_json(&self) -> Json {
        json!({
            "api": self.api,
            "bytes": self.bytes,
            "duration_ms": self.duration_ms,
            "error": self.error,
        })
    }
}

/// Shared per-request file call chain. The worker thread appends; the request
/// handler reads it even when a timeout orphans the worker.
pub type CallLog = Arc<Mutex<Vec<CallRecord>>>;

/// Snapshot a request's file call chain for the structured log.
#[must_use]
pub fn calls_json(calls: &CallLog) -> Vec<Json> {
    calls.lock().map_or_else(
        |_| Vec::new(),
        |calls| calls.iter().map(CallRecord::to_json).collect(),
    )
}

/// `Content-Type` media type that selects the multipart parser.
const MULTIPART_CONTENT_TYPE: &str = "multipart/form-data";

/// Whether a request body should be parsed as multipart form data.
#[must_use]
pub fn is_multipart(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(MULTIPART_CONTENT_TYPE))
}

/// Parsed `Content-Length`, when the client supplied a valid one.
#[must_use]
pub fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Script-visible metadata for one uploaded file. The temporary path is never
/// part of this type.
#[derive(Debug, Clone)]
pub struct UploadFileMeta {
    pub field: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: u64,
}

impl UploadFileMeta {
    #[must_use]
    pub fn to_json(&self) -> Json {
        json!({
            "field": self.field,
            "filename": self.filename,
            "contentType": self.content_type,
            "size": self.size,
        })
    }
}

#[derive(Debug)]
struct UploadedFile {
    meta: UploadFileMeta,
    path: PathBuf,
}

/// One request's uploaded files plus the guard that removes their random
/// temporary directory when the last owner drops it.
#[derive(Debug)]
pub struct UploadStore {
    dir: Mutex<Option<TempDir>>,
    files: Vec<UploadedFile>,
    pub total_bytes: u64,
}

impl UploadStore {
    /// Clone the script-visible metadata in upload order.
    #[must_use]
    pub fn metas(&self) -> Vec<UploadFileMeta> {
        self.files.iter().map(|file| file.meta.clone()).collect()
    }

    fn file(&self, index: usize) -> Option<&UploadedFile> {
        self.files.get(index)
    }

    /// Delete the temporary contents immediately while retaining the guard.
    /// Used when a timed-out worker may still hold an `Arc` after the client
    /// response is decided. On Windows an already-open upload file can block
    /// deletion; keeping the guard lets its `Drop` retry once that worker
    /// releases the handle.
    pub fn close(&self) {
        let dir = self
            .dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dir) = dir.as_ref() {
            let _ = std::fs::remove_dir_all(dir.path());
        }
    }
}

/// Stable failure from parsing a multipart request before any script runs.
#[derive(Debug, Clone, Copy)]
pub enum ParseFailure {
    Invalid { files: usize, total_bytes: u64 },
    TooLarge { files: usize, total_bytes: u64 },
    Io { files: usize, total_bytes: u64 },
}

impl ParseFailure {
    #[must_use]
    pub fn too_large(files: usize, total_bytes: u64) -> Self {
        Self::TooLarge { files, total_bytes }
    }

    fn invalid(files: usize, total_bytes: u64) -> Self {
        Self::Invalid { files, total_bytes }
    }

    fn io(files: usize, total_bytes: u64) -> Self {
        Self::Io { files, total_bytes }
    }

    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid { .. } => "invalid_multipart",
            Self::TooLarge { .. } => "upload_too_large",
            Self::Io { .. } => "upload_io_error",
        }
    }

    #[must_use]
    pub fn files(self) -> usize {
        match self {
            Self::Invalid { files, .. } | Self::TooLarge { files, .. } | Self::Io { files, .. } => {
                files
            }
        }
    }

    #[must_use]
    pub fn total_bytes(self) -> u64 {
        match self {
            Self::Invalid { total_bytes, .. }
            | Self::TooLarge { total_bytes, .. }
            | Self::Io { total_bytes, .. } => total_bytes,
        }
    }

    #[must_use]
    pub fn status_code(self) -> u16 {
        match self {
            Self::TooLarge { .. } => 413,
            Self::Invalid { .. } => 400,
            Self::Io { .. } => 500,
        }
    }

    /// Client-visible diagnostic under `--verbose`: a stable class only.
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "upload exceeds files.upload_max_bytes",
            Self::Invalid { .. } => "multipart body is malformed",
            Self::Io { .. } => "upload temporary storage failed",
        }
    }
}

/// Parse a multipart request into a request-scoped temporary directory.
///
/// File data and non-file field data share the configured byte budget;
/// multipart framing overhead is not counted. The client filename is never
/// used for a path.
pub async fn parse_multipart(
    request: Request,
    max_bytes: u64,
) -> Result<Arc<UploadStore>, ParseFailure> {
    let temp = create_upload_dir().map_err(|_| ParseFailure::io(0, 0))?;
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|_| ParseFailure::invalid(0, 0))?;
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| map_multipart_error(&error, files.len(), total_bytes))?
    {
        let field_name = field.name().map(str::to_string);
        let filename = field.file_name().map(client_basename);
        let content_type = field.content_type().map(str::to_string);
        let is_file = field_name.is_some() && filename.is_some();
        // Opaque numbered names: a client filename never reaches the disk.
        let path = is_file.then(|| temp.path().join(files.len().to_string()));
        let mut writer = match path.as_deref() {
            Some(path) => Some(
                tokio::fs::File::create(path)
                    .await
                    .map_err(|_| ParseFailure::io(files.len(), total_bytes))?,
            ),
            None => None,
        };
        let mut size = 0_u64;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| map_multipart_error(&error, files.len(), total_bytes))?
        {
            let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            total_bytes = total_bytes.saturating_add(chunk_len);
            if total_bytes > max_bytes {
                return Err(ParseFailure::too_large(files.len(), total_bytes));
            }
            if let Some(writer) = writer.as_mut() {
                writer
                    .write_all(&chunk)
                    .await
                    .map_err(|_| ParseFailure::io(files.len(), total_bytes))?;
                size = size.saturating_add(chunk_len);
            }
        }
        if let Some(mut writer) = writer {
            writer
                .flush()
                .await
                .map_err(|_| ParseFailure::io(files.len(), total_bytes))?;
            writer
                .shutdown()
                .await
                .map_err(|_| ParseFailure::io(files.len(), total_bytes))?;
        }
        if is_file {
            files.push(UploadedFile {
                meta: UploadFileMeta {
                    field: field_name.unwrap_or_default(),
                    filename: filename.unwrap_or_default(),
                    content_type,
                    size,
                },
                path: path.expect("file fields have a temporary path"),
            });
        }
    }
    Ok(Arc::new(UploadStore {
        dir: Mutex::new(Some(temp)),
        files,
        total_bytes,
    }))
}

/// Create the random request-scoped upload directory. Unix permissions are
/// pinned to owner-only; other platforms use the host's default ACL.
#[cfg(unix)]
fn create_upload_dir() -> std::io::Result<TempDir> {
    use std::os::unix::fs::PermissionsExt as _;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
}

#[cfg(not(unix))]
fn create_upload_dir() -> std::io::Result<TempDir> {
    tempfile::tempdir()
}

/// Classify one multer/axum parser failure without exposing parser text.
fn map_multipart_error(
    error: &axum::extract::multipart::MultipartError,
    files: usize,
    total_bytes: u64,
) -> ParseFailure {
    if error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        ParseFailure::too_large(files, total_bytes)
    } else {
        ParseFailure::invalid(files, total_bytes)
    }
}

/// Strip both slash styles; only the client-provided basename is metadata.
fn client_basename(raw: &str) -> String {
    raw.rsplit(['/', '\\']).next().unwrap_or(raw).to_string()
}

/// One script-visible `ctx.request.files[i]` operation.
#[derive(Debug, Clone, Copy)]
enum UploadOp {
    ReadText,
    ReadBytes,
    Stream,
}

impl UploadOp {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "readText" => Some(Self::ReadText),
            "readBytes" => Some(Self::ReadBytes),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }

    fn api(self) -> &'static str {
        match self {
            Self::ReadText => "upload.text",
            Self::ReadBytes => "upload.bytes",
            Self::Stream => "upload.stream",
        }
    }
}

/// Request-scoped access to uploaded files. It shares the file call chain with
/// `ctx.file`, so operator logs keep one ordered list.
#[derive(Debug)]
pub struct UploadAccess {
    store: Arc<UploadStore>,
    calls: CallLog,
    streams: RefCell<Vec<Option<FileBody>>>,
}

impl UploadAccess {
    #[must_use]
    pub fn new(store: Arc<UploadStore>, calls: CallLog) -> Self {
        Self {
            store,
            calls,
            streams: RefCell::new(Vec::new()),
        }
    }

    /// Handle one bridge call. Payload: `{op, index}`.
    #[must_use]
    pub fn call(&self, payload: &Json) -> Json {
        let Some(raw_op) = payload.get("op").and_then(Json::as_str) else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.request.files: missing operation"
            });
        };
        let Some(op) = UploadOp::parse(raw_op) else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.request.files: unknown operation"
            });
        };
        let Some(index) = payload
            .get("index")
            .and_then(Json::as_u64)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.request.files: index must be a non-negative integer"
            });
        };
        let Some(uploaded) = self.store.file(index) else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.request.files: index out of range"
            });
        };
        let call_index = begin_call(&self.calls, op.api());
        let started = Instant::now();
        let read = match op {
            UploadOp::ReadText => read_upload_capped(&uploaded.path)
                .and_then(|bytes| String::from_utf8(bytes).map_err(|_| Error::Encoding))
                .map(|text| {
                    let payload = json!({ "ok": true, "text": text });
                    (payload, text.len())
                }),
            UploadOp::ReadBytes => read_upload_capped(&uploaded.path).map(|bytes| {
                let len = bytes.len();
                let payload = json!({ "ok": true, "bytes_base64": BASE64.encode(&bytes) });
                (payload, len)
            }),
            UploadOp::Stream => {
                return match self.open_stream(&uploaded.path, call_index, started) {
                    Ok(handle) => json!({ "ok": true, "handle": handle }),
                    Err(error) => error_json(&error),
                };
            }
        };
        match read {
            Ok((payload, bytes)) => {
                finish_call(
                    &self.calls,
                    call_index,
                    started,
                    u64::try_from(bytes).ok(),
                    None,
                );
                payload
            }
            Err(error) => {
                finish_call(&self.calls, call_index, started, None, Some(error.code()));
                error_json(&error)
            }
        }
    }

    /// Open one uploaded file as a streamed Response body. The guard keeps the
    /// request-scoped directory alive until the relay finishes or disconnects.
    fn open_stream(&self, path: &Path, call_index: usize, started: Instant) -> Result<u64, Error> {
        let metadata = std::fs::metadata(path).map_err(|error| map_open_error(&error))?;
        if !metadata.is_file() {
            let error = Error::Io("path is not a regular file".to_string());
            finish_call(&self.calls, call_index, started, None, Some(error.code()));
            return Err(error);
        }
        let file = File::open(path).map_err(|error| map_open_error(&error))?;
        let size = metadata.len();
        let call = CallHandle::new(Arc::clone(&self.calls), call_index, started);
        let body = FileBody {
            file,
            size,
            offset: 0,
            len: size,
            call,
            guard: Some(Arc::clone(&self.store)),
        };
        Ok(register_stream(&self.streams, body))
    }

    /// Claim one streamed body for the first Response that used it.
    #[must_use]
    pub fn take_stream(&self, handle: u64) -> Option<FileBody> {
        let index = usize::try_from(handle).ok()?;
        self.streams.borrow_mut().get_mut(index)?.take()
    }
}

/// Read one host-owned uploaded file under the shared buffered cap.
fn read_upload_capped(path: &Path) -> Result<Vec<u8>, Error> {
    let metadata = std::fs::metadata(path).map_err(|error| map_open_error(&error))?;
    if !metadata.is_file() {
        return Err(Error::Io("path is not a regular file".to_string()));
    }
    let file = File::open(path).map_err(|error| map_open_error(&error))?;
    read_capped_file(file, metadata.len())
}

/// Register one opened body under a fresh opaque stream handle.
fn register_stream(streams: &RefCell<Vec<Option<FileBody>>>, body: FileBody) -> u64 {
    let mut streams = streams.borrow_mut();
    streams.push(Some(body));
    u64::try_from(streams.len().saturating_sub(1)).unwrap_or(u64::MAX)
}

fn read_capped_file(file: File, size: u64) -> Result<Vec<u8>, Error> {
    if size > MAX_READ_BYTES {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.take(MAX_READ_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Io(error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_READ_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

/// Append one file call and return its slot in the shared per-request log.
fn begin_call(calls: &CallLog, api: &'static str) -> usize {
    let mut calls = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    calls.push(CallRecord::new(api));
    calls.len().saturating_sub(1)
}

fn finish_call(
    calls: &CallLog,
    index: usize,
    started: Instant,
    bytes: Option<u64>,
    error: Option<&'static str>,
) {
    let mut calls = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(record) = calls.get_mut(index) else {
        return;
    };
    record.bytes = bytes;
    record.duration_ms = Some(elapsed_ms(started.elapsed()));
    record.error = error;
}

/// Request-scoped access rooted at the configured static file root.
#[derive(Debug)]
pub struct FileAccess {
    root: PathBuf,
    calls: CallLog,
    /// Opened `ctx.file.stream` bodies, claimed by the Response that uses them.
    streams: RefCell<Vec<Option<FileBody>>>,
}

impl FileAccess {
    /// Record the configured root. The root is canonicalized per call, so a
    /// root that disappears or stops being a directory at runtime surfaces as
    /// a catchable `file_io_error` instead of failing the script before it runs.
    #[must_use]
    pub fn new(root: &Path, calls: CallLog) -> Self {
        Self {
            root: root.to_path_buf(),
            calls,
            streams: RefCell::new(Vec::new()),
        }
    }

    /// Handle one `ctx.file.*` bridge call. Payload: `{op, path}`.
    #[must_use]
    pub fn call(&self, payload: &Json) -> Json {
        let Some(raw_op) = payload.get("op").and_then(Json::as_str) else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.file: missing operation"
            });
        };
        let Some(path) = payload.get("path").and_then(Json::as_str) else {
            return json!({
                "ok": false,
                "code": "file_path_invalid",
                "message": "ctx.file: path must be a string"
            });
        };
        let Some(op) = FileOp::parse(raw_op) else {
            return json!({
                "ok": false,
                "code": "script_error",
                "message": "ctx.file: unknown operation"
            });
        };
        let index = begin_call(&self.calls, op.api());
        let started = Instant::now();
        let read = match op {
            FileOp::ReadText => self.read_text(path).map(|text| {
                let payload = json!({ "ok": true, "text": text });
                (payload, text.len())
            }),
            FileOp::ReadBytes => self.read_bytes(path).map(|bytes| {
                let len = bytes.len();
                let payload = json!({ "ok": true, "bytes_base64": BASE64.encode(&bytes) });
                (payload, len)
            }),
            FileOp::Stream => {
                return match self.open_stream(path, index, started) {
                    Ok(handle) => json!({ "ok": true, "handle": handle }),
                    Err(error) => error_json(&error),
                };
            }
        };
        match read {
            Ok((payload, bytes)) => {
                finish_call(&self.calls, index, started, u64::try_from(bytes).ok(), None);
                payload
            }
            Err(error) => {
                finish_call(&self.calls, index, started, None, Some(error.code()));
                error_json(&error)
            }
        }
    }

    /// Open one `ctx.file.stream` body and register it under a fresh handle.
    fn open_stream(&self, path: &str, index: usize, started: Instant) -> Result<u64, Error> {
        let (file, size) = match self.open(path) {
            Ok(opened) => opened,
            Err(error) => {
                finish_call(&self.calls, index, started, None, Some(error.code()));
                return Err(error);
            }
        };
        let call = CallHandle::new(Arc::clone(&self.calls), index, started);
        let body = FileBody {
            file,
            size,
            offset: 0,
            len: size,
            call,
            guard: None,
        };
        Ok(register_stream(&self.streams, body))
    }

    /// Claim one streamed body for the first Response that used it. Reusing a
    /// handle finds nothing, which keeps it single-consumption.
    #[must_use]
    pub fn take_stream(&self, handle: u64) -> Option<FileBody> {
        let index = usize::try_from(handle).ok()?;
        self.streams.borrow_mut().get_mut(index)?.take()
    }

    fn read_text(&self, path: &str) -> Result<String, Error> {
        let bytes = self.read_capped(path)?;
        String::from_utf8(bytes).map_err(|_| Error::Encoding)
    }

    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, Error> {
        self.read_capped(path)
    }

    fn read_capped(&self, path: &str) -> Result<Vec<u8>, Error> {
        let (file, size) = self.open(path)?;
        read_capped_file(file, size)
    }

    fn open(&self, raw: &str) -> Result<(File, u64), Error> {
        let (target, root) = self.resolve(raw)?;
        // Check the path type before opening: a FIFO would block `File::open`
        // until a writer appears, which must surface as a `file_io_error`
        // instead of hanging the script past its deadline.
        let metadata = std::fs::metadata(&target).map_err(|error| map_open_error(&error))?;
        if !metadata.is_file() {
            return Err(Error::Io("path is not a regular file".to_string()));
        }
        let file = File::open(&target).map_err(|error| map_open_error(&error))?;
        let opened = file
            .metadata()
            .map_err(|error| Error::Io(error.to_string()))?;
        if !opened.is_file() {
            return Err(Error::Io("path is not a regular file".to_string()));
        }
        self.verify_open_target(&file, &root)?;
        Ok((file, opened.len()))
    }

    /// Resolve one script-supplied path inside the root with component-based
    /// checks; canonicalization then rejects symlink escapes. Returns the
    /// canonical target and the canonical root it was checked against.
    fn resolve(&self, raw: &str) -> Result<(PathBuf, PathBuf), Error> {
        if raw.is_empty() {
            return Err(Error::PathInvalid("path must not be empty"));
        }
        for component in Path::new(raw).components() {
            match component {
                Component::ParentDir => {
                    return Err(Error::PathInvalid("`..` components are not allowed"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(Error::PathInvalid("absolute paths are not allowed"));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        let root =
            std::fs::canonicalize(&self.root).map_err(|error| Error::Io(error.to_string()))?;
        if !root.is_dir() {
            return Err(Error::Io("static file root is not a directory".to_string()));
        }
        let target = std::fs::canonicalize(root.join(raw)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound
            } else {
                Error::Io(error.to_string())
            }
        })?;
        if !target.starts_with(&root) {
            return Err(Error::PathInvalid("path escapes the static file root"));
        }
        Ok((target, root))
    }

    /// Best-effort post-open re-check. On Linux the file descriptor names the
    /// object actually opened, which closes most of the resolution window;
    /// other platforms keep the pre-open check only.
    #[cfg(target_os = "linux")]
    fn verify_open_target(&self, file: &File, root: &Path) -> Result<(), Error> {
        use std::os::fd::AsRawFd;

        let Ok(actual) = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) else {
            return Ok(());
        };
        let Ok(canonical) = std::fs::canonicalize(actual) else {
            return Ok(());
        };
        if canonical.starts_with(root) {
            Ok(())
        } else {
            Err(Error::PathInvalid("path escapes the static file root"))
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn verify_open_target(&self, _file: &File, _root: &Path) -> Result<(), Error> {
        Ok(())
    }
}

fn map_open_error(error: &std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::NotFound
    } else {
        Error::Io(error.to_string())
    }
}

fn elapsed_ms(elapsed: Duration) -> f64 {
    let millis = elapsed.as_secs_f64() * 1000.0;
    (millis * 100.0).round() / 100.0
}

/// Single-range decision for one streamed file Response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeDecision {
    /// No `Range` header: answer the whole body with 200.
    Full,
    /// A satisfiable single range: answer 206 for `[start, start + len)`.
    Partial { start: u64, len: u64 },
    /// Malformed, multi-range, empty-file, or unsatisfiable: answer 416.
    Unsatisfiable,
}

/// Decide the single-range Response for one file size and `Range` value. End
/// offsets clamp to the file end; every unusable value is unsatisfiable.
#[must_use]
pub fn decide_range(size: u64, range: Option<&str>) -> RangeDecision {
    let Some(header) = range else {
        return RangeDecision::Full;
    };
    // Only the surrounding OWS allowed on a field value is dropped; interior
    // whitespace is not valid in a range spec and must answer 416.
    let Some((unit, spec)) = header.trim().split_once('=') else {
        return RangeDecision::Unsatisfiable;
    };
    if !unit.eq_ignore_ascii_case("bytes") || spec.contains(',') || size == 0 {
        return RangeDecision::Unsatisfiable;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeDecision::Unsatisfiable;
    };
    if first.is_empty() {
        // suffix-byte-range-spec: `bytes=-N` asks for the last N bytes.
        let Some(suffix) = parse_index(last) else {
            return RangeDecision::Unsatisfiable;
        };
        if suffix == 0 {
            return RangeDecision::Unsatisfiable;
        }
        let len = suffix.min(size);
        return RangeDecision::Partial {
            start: size - len,
            len,
        };
    }
    let Some(start) = parse_index(first) else {
        return RangeDecision::Unsatisfiable;
    };
    if start >= size {
        return RangeDecision::Unsatisfiable;
    }
    let end = if last.is_empty() {
        size - 1
    } else {
        let Some(end) = parse_index(last) else {
            return RangeDecision::Unsatisfiable;
        };
        if end < start {
            return RangeDecision::Unsatisfiable;
        }
        end.min(size - 1)
    };
    RangeDecision::Partial {
        start,
        len: end - start + 1,
    }
}

fn parse_index(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

/// One opened `ctx.file.stream` body, owned by the Response that claimed it.
#[derive(Debug)]
pub struct FileBody {
    pub file: File,
    pub size: u64,
    pub offset: u64,
    pub len: u64,
    pub call: CallHandle,
    /// Uploaded bodies keep the temporary directory alive until the relay
    /// finishes; rooted `ctx.file.stream` bodies leave this empty.
    pub guard: Option<Arc<UploadStore>>,
}

/// How a streamed file body ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    Complete,
    /// The read failed or the file ended before the announced length.
    FileError,
    ClientDisconnected,
}

impl StreamOutcome {
    /// Classify a relay channel that closed before the announced length was
    /// delivered; a satisfied length is a completed delivery.
    #[must_use]
    pub fn from_channel_close(bytes: u64, announced_content_length: Option<u64>) -> Self {
        if announced_content_length == Some(bytes) {
            Self::Complete
        } else {
            Self::ClientDisconnected
        }
    }

    /// Request-log error class when the stream did not complete normally.
    #[must_use]
    pub fn error_class(self) -> Option<&'static str> {
        match self {
            Self::Complete => None,
            Self::FileError => Some("file_stream_error"),
            Self::ClientDisconnected => Some("client_disconnected"),
        }
    }
}

/// Handle that finalizes one `ctx.file.stream` call when its body ends, or
/// when the script discards the handle without using it.
#[derive(Debug)]
pub struct CallHandle {
    calls: CallLog,
    index: usize,
    started: Instant,
    finished: AtomicBool,
}

impl CallHandle {
    fn new(calls: CallLog, index: usize, started: Instant) -> Self {
        Self {
            calls,
            index,
            started,
            finished: AtomicBool::new(false),
        }
    }

    /// Finalize this call with the outcome of its body stream.
    pub fn finish(&self, outcome: StreamOutcome, bytes: u64) {
        self.finalize(Some(outcome), bytes);
    }

    fn finalize(&self, outcome: Option<StreamOutcome>, bytes: u64) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(record) = calls.get_mut(self.index) else {
            return;
        };
        record.duration_ms = Some(elapsed_ms(self.started.elapsed()));
        record.bytes = Some(bytes);
        if let Some(outcome) = outcome {
            record.error = outcome.error_class();
        }
    }

    /// Snapshot the call chain after this stream finalized it.
    #[must_use]
    pub fn calls_json(&self) -> Vec<Json> {
        calls_json(&self.calls)
    }
}

impl Drop for CallHandle {
    fn drop(&mut self) {
        self.finalize(None, 0);
    }
}

fn error_json(error: &Error) -> Json {
    json!({
        "ok": false,
        "code": error.code(),
        "message": error.message(),
    })
}
