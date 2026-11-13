//! Rooted local file access for `ctx.file` plus request-scoped uploads.
//! Contracts: docs/contracts/ctx-api.md, docs/contracts/config.md
//!
//! Every script-visible path resolves against the configured file root.
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
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{Read, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::request_log::{elapsed_ms, Chain, LogRecord, Pending};

use serde_json::{json, Value as Json};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt as _;

/// Buffered read cap shared by `ctx.file.readText` and `ctx.file.readBytes`.
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Buffered write cap shared by `ctx.file.writeText` and `ctx.file.writeBytes`.
const MAX_WRITE_BYTES: u64 = 8 * 1024 * 1024;

/// One script-visible `ctx.file` operation.
#[derive(Debug, Clone, Copy)]
enum FileOp {
    ReadText,
    ReadBytes,
    Stream,
    WriteText,
    WriteBytes,
}

impl FileOp {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "readText" => Some(Self::ReadText),
            "readBytes" => Some(Self::ReadBytes),
            "stream" => Some(Self::Stream),
            "writeText" => Some(Self::WriteText),
            "writeBytes" => Some(Self::WriteBytes),
            _ => None,
        }
    }
}

/// One decoded `ctx.file` call.
#[allow(clippy::enum_variant_names)]
pub(crate) enum FileCall {
    ReadText {
        path: String,
    },
    ReadBytes {
        path: String,
    },
    Stream {
        path: String,
    },
    WriteText {
        path: String,
        text: Option<String>,
    },
    WriteBytes {
        path: String,
        bytes_base64: Option<String>,
    },
}

impl FileCall {
    /// Decode one bridge payload without claiming a file call-log slot.
    pub(crate) fn parse(payload: &Json) -> Result<Self, Error> {
        let Some(raw_op) = payload.get("op").and_then(Json::as_str) else {
            return Err(Error::Script("ctx.file: missing operation"));
        };
        let Some(path) = payload.get("path").and_then(Json::as_str) else {
            return Err(Error::PathInvalid("ctx.file: path must be a string"));
        };
        let Some(op) = FileOp::parse(raw_op) else {
            return Err(Error::Script("ctx.file: unknown operation"));
        };
        Ok(match op {
            FileOp::ReadText => Self::ReadText {
                path: path.to_string(),
            },
            FileOp::ReadBytes => Self::ReadBytes {
                path: path.to_string(),
            },
            FileOp::Stream => Self::Stream {
                path: path.to_string(),
            },
            FileOp::WriteText => Self::WriteText {
                path: path.to_string(),
                text: payload
                    .get("text")
                    .and_then(Json::as_str)
                    .map(str::to_string),
            },
            FileOp::WriteBytes => Self::WriteBytes {
                path: path.to_string(),
                bytes_base64: payload
                    .get("bytes_base64")
                    .and_then(Json::as_str)
                    .map(str::to_string),
            },
        })
    }

    const fn api(&self) -> &'static str {
        match self {
            Self::ReadText { .. } => "file.readText",
            Self::ReadBytes { .. } => "file.readBytes",
            Self::Stream { .. } => "file.stream",
            Self::WriteText { .. } => "file.writeText",
            Self::WriteBytes { .. } => "file.writeBytes",
        }
    }
}

/// One successful file or upload operation, before wire encoding.
pub(crate) enum FileReply {
    Text(String),
    Bytes(Vec<u8>),
    Stream(u64),
    Written(u64),
}

/// Why one `ctx.file` call failed. Codes are stable script-visible values.
#[derive(Debug)]
pub enum Error {
    PathInvalid(&'static str),
    NotFound,
    TooLarge,
    WriteDisabled,
    WriteTooLarge(&'static str),
    Script(&'static str),
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
            Self::TooLarge | Self::WriteTooLarge(_) => "file_too_large",
            Self::WriteDisabled => "file_write_disabled",
            Self::Script(_) => "script_error",
            Self::Encoding => "file_encoding_error",
            Self::Io(_) => "file_io_error",
        }
    }

    /// Operator-facing reason; never carries a file path.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::PathInvalid(reason) | Self::WriteTooLarge(reason) | Self::Script(reason) => {
                (*reason).to_string()
            }
            Self::NotFound => "file not found".to_string(),
            Self::TooLarge => format!("file exceeds the {MAX_READ_BYTES}-byte read cap"),
            Self::WriteDisabled => {
                "script file writes are disabled; set [files] writable = true".to_string()
            }
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
}

impl LogRecord for CallRecord {
    /// Log shape shared by every file call. Paths never enter it.
    fn to_json(&self) -> Json {
        json!({
            "api": self.api,
            "bytes": self.bytes,
            "duration_ms": self.duration_ms,
            "error": self.error,
        })
    }

    fn set_duration_ms(&mut self, ms: f64) {
        self.duration_ms = Some(ms);
    }

    fn set_bytes(&mut self, bytes: u64) {
        self.bytes = Some(bytes);
    }
}

/// Shared per-request file call chain. The worker thread appends; the request
/// handler reads it even when a timeout orphans the worker.
pub type CallLog = Chain<CallRecord>;

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
    Invalid {
        files: usize,
        counted_bytes: Option<u64>,
    },
    TooLarge {
        files: usize,
        counted_bytes: Option<u64>,
    },
    Io {
        files: usize,
        counted_bytes: Option<u64>,
    },
}

impl ParseFailure {
    #[must_use]
    pub fn too_large(files: usize, counted_bytes: Option<u64>) -> Self {
        Self::TooLarge {
            files,
            counted_bytes,
        }
    }

    fn invalid(files: usize, counted_bytes: Option<u64>) -> Self {
        Self::Invalid {
            files,
            counted_bytes,
        }
    }

    fn io(files: usize, counted_bytes: Option<u64>) -> Self {
        Self::Io {
            files,
            counted_bytes,
        }
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

    /// Field data already counted when the failure occurred. `None` means no
    /// part body had been read yet; `Some(0)` is a real zero-byte count.
    #[must_use]
    pub fn counted_bytes(self) -> Option<u64> {
        match self {
            Self::Invalid { counted_bytes, .. }
            | Self::TooLarge { counted_bytes, .. }
            | Self::Io { counted_bytes, .. } => counted_bytes,
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
    let temp = create_upload_dir().map_err(|_| ParseFailure::io(0, None))?;
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|_| ParseFailure::invalid(0, None))?;
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    let mut counted_bytes = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| map_multipart_error(&error, files.len(), counted_bytes))?
    {
        let Some(field_name) = field.name().map(str::to_string) else {
            return Err(ParseFailure::invalid(files.len(), counted_bytes));
        };
        let filename = field.file_name().map(client_basename);
        let content_type = field.content_type().map(str::to_string);
        let is_file = filename.is_some();
        // Opaque numbered names: a client filename never reaches the disk.
        let path = is_file.then(|| temp.path().join(files.len().to_string()));
        let mut writer = match path.as_deref() {
            Some(path) => Some(
                tokio::fs::File::create(path)
                    .await
                    .map_err(|_| ParseFailure::io(files.len(), counted_bytes))?,
            ),
            None => None,
        };
        let mut size = 0_u64;
        // From here on, zero bytes is a real count rather than "not read".
        counted_bytes = Some(total_bytes);
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| map_multipart_error(&error, files.len(), counted_bytes))?
        {
            let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            total_bytes = total_bytes.saturating_add(chunk_len);
            counted_bytes = Some(total_bytes);
            if total_bytes > max_bytes {
                return Err(ParseFailure::too_large(files.len(), counted_bytes));
            }
            if let Some(writer) = writer.as_mut() {
                writer
                    .write_all(&chunk)
                    .await
                    .map_err(|_| ParseFailure::io(files.len(), counted_bytes))?;
                size = size.saturating_add(chunk_len);
            }
        }
        if let Some(mut writer) = writer {
            writer
                .flush()
                .await
                .map_err(|_| ParseFailure::io(files.len(), counted_bytes))?;
            writer
                .shutdown()
                .await
                .map_err(|_| ParseFailure::io(files.len(), counted_bytes))?;
        }
        if is_file {
            files.push(UploadedFile {
                meta: UploadFileMeta {
                    field: field_name,
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
    counted_bytes: Option<u64>,
) -> ParseFailure {
    if error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        ParseFailure::too_large(files, counted_bytes)
    } else {
        ParseFailure::invalid(files, counted_bytes)
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

/// One decoded `ctx.request.files[i]` call.
pub(crate) struct UploadCall {
    op: UploadOp,
    path: PathBuf,
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

    /// Decode one bridge payload and resolve the upload it names.
    pub(crate) fn prepare(&self, payload: &Json) -> Result<UploadCall, Error> {
        let Some(raw_op) = payload.get("op").and_then(Json::as_str) else {
            return Err(Error::Script("ctx.request.files: missing operation"));
        };
        let Some(op) = UploadOp::parse(raw_op) else {
            return Err(Error::Script("ctx.request.files: unknown operation"));
        };
        let Some(index) = payload
            .get("index")
            .and_then(Json::as_u64)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return Err(Error::Script(
                "ctx.request.files: index must be a non-negative integer",
            ));
        };
        let Some(uploaded) = self.store.file(index) else {
            return Err(Error::Script("ctx.request.files: index out of range"));
        };
        Ok(UploadCall {
            op,
            path: uploaded.path.clone(),
        })
    }

    /// Handle one `ctx.request.files[i]` JSON bridge call.
    #[cfg(test)]
    #[must_use]
    pub fn call(&self, payload: &Json) -> Json {
        match self.prepare(payload).and_then(|call| self.execute(&call)) {
            Ok(reply) => reply_json(reply),
            Err(error) => error_json(&error),
        }
    }

    /// Execute one decoded upload call.
    pub(crate) fn execute(&self, call: &UploadCall) -> Result<FileReply, Error> {
        let call_index = begin_call(&self.calls, call.op.api());
        let started = Instant::now();
        let read = match call.op {
            UploadOp::ReadText => read_upload_capped(&call.path)
                .and_then(|bytes| String::from_utf8(bytes).map_err(|_| Error::Encoding))
                .map(|text| {
                    let len = text.len();
                    (FileReply::Text(text), len)
                }),
            UploadOp::ReadBytes => read_upload_capped(&call.path).map(|bytes| {
                let len = bytes.len();
                (FileReply::Bytes(bytes), len)
            }),
            UploadOp::Stream => {
                return match self.open_stream(&call.path, call_index, started) {
                    Ok(handle) => Ok(FileReply::Stream(handle)),
                    Err(error) => {
                        finish_call(&self.calls, call_index, started, None, Some(error.code()));
                        Err(error)
                    }
                };
            }
        };
        match read {
            Ok((reply, bytes)) => {
                finish_call(
                    &self.calls,
                    call_index,
                    started,
                    u64::try_from(bytes).ok(),
                    None,
                );
                Ok(reply)
            }
            Err(error) => {
                finish_call(&self.calls, call_index, started, None, Some(error.code()));
                Err(error)
            }
        }
    }

    /// Open one uploaded file as a streamed Response body. The guard keeps the
    /// request-scoped directory alive until the relay finishes or disconnects.
    fn open_stream(&self, path: &Path, call_index: usize, started: Instant) -> Result<u64, Error> {
        let metadata = std::fs::metadata(path).map_err(|error| map_open_error(&error))?;
        if !metadata.is_file() {
            return Err(Error::Io("path is not a regular file".to_string()));
        }
        let file = File::open(path).map_err(|error| map_open_error(&error))?;
        let size = metadata.len();
        let call = Pending::new(self.calls.clone(), call_index, started);
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
    calls.begin(CallRecord::new(api))
}

fn finish_call(
    calls: &CallLog,
    index: usize,
    started: Instant,
    bytes: Option<u64>,
    error: Option<&'static str>,
) {
    let ms = elapsed_ms(started.elapsed());
    calls.update(index, |record| {
        record.bytes = bytes;
        record.duration_ms = Some(ms);
        record.error = error;
    });
}

/// Request-scoped access rooted at the configured file root.
#[derive(Debug)]
pub struct FileAccess {
    root: PathBuf,
    writable: bool,
    write_max_bytes: u64,
    /// Script deadline; a replacement that cannot finish before it must not
    /// keep retrying, because a timed-out request must not leave side effects.
    write_deadline: Instant,
    /// Bytes successfully written by this request; the host, not the script,
    /// owns this budget.
    written_bytes: Cell<u64>,
    calls: CallLog,
    /// Opened `ctx.file.stream` bodies, claimed by the Response that uses them.
    streams: RefCell<Vec<Option<FileBody>>>,
}

impl FileAccess {
    /// Record the configured root. The root is canonicalized per call, so a
    /// root that disappears or stops being a directory at runtime surfaces as
    /// a catchable `file_io_error` instead of failing the script before it runs.
    #[must_use]
    pub fn new(
        root: &Path,
        writable: bool,
        write_max_bytes: u64,
        write_deadline: Instant,
        calls: CallLog,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            writable,
            write_max_bytes,
            write_deadline,
            written_bytes: Cell::new(0),
            calls,
            streams: RefCell::new(Vec::new()),
        }
    }

    /// Handle one `ctx.file.*` JSON bridge call.
    #[cfg(test)]
    #[must_use]
    pub fn call(&self, payload: &Json) -> Json {
        match FileCall::parse(payload).and_then(|call| self.execute(call)) {
            Ok(reply) => reply_json(reply),
            Err(error) => error_json(&error),
        }
    }

    /// Execute one decoded `ctx.file` call.
    pub(crate) fn execute(&self, call: FileCall) -> Result<FileReply, Error> {
        let index = begin_call(&self.calls, call.api());
        let started = Instant::now();
        let operation = match call {
            FileCall::ReadText { path } => self.read_text(&path).map(|text| {
                let len = u64::try_from(text.len()).unwrap_or(u64::MAX);
                (FileReply::Text(text), len)
            }),
            FileCall::ReadBytes { path } => self.read_bytes(&path).map(|bytes| {
                let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                (FileReply::Bytes(bytes), len)
            }),
            FileCall::Stream { path } => {
                return match self.open_stream(&path, index, started) {
                    Ok(handle) => Ok(FileReply::Stream(handle)),
                    Err(error) => {
                        finish_call(&self.calls, index, started, None, Some(error.code()));
                        Err(error)
                    }
                };
            }
            FileCall::WriteText { path, text } => text
                .ok_or(Error::Script("ctx.file.writeText: text must be a string"))
                .and_then(|text| self.write_text(&path, &text))
                .map(|len| (FileReply::Written(len), len)),
            FileCall::WriteBytes { path, bytes_base64 } => bytes_base64
                .ok_or(Error::Script("ctx.file.writeBytes: bytes must be provided"))
                .and_then(|encoded| {
                    BASE64
                        .decode(&encoded)
                        .map_err(|_| Error::Script("ctx.file.writeBytes: invalid byte payload"))
                })
                .and_then(|bytes| self.write_bytes(&path, &bytes))
                .map(|len| (FileReply::Written(len), len)),
        };
        match operation {
            Ok((reply, bytes)) => {
                finish_call(&self.calls, index, started, Some(bytes), None);
                Ok(reply)
            }
            Err(error) => {
                finish_call(&self.calls, index, started, None, Some(error.code()));
                Err(error)
            }
        }
    }

    /// Open one `ctx.file.stream` body and register it under a fresh handle.
    fn open_stream(&self, path: &str, index: usize, started: Instant) -> Result<u64, Error> {
        let (file, size) = self.open(path)?;
        let call = Pending::new(self.calls.clone(), index, started);
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

    fn write_text(&self, path: &str, text: &str) -> Result<u64, Error> {
        self.write_bytes(path, text.as_bytes())
    }

    fn write_bytes(&self, path: &str, bytes: &[u8]) -> Result<u64, Error> {
        if !self.writable {
            return Err(Error::WriteDisabled);
        }
        let len = u64::try_from(bytes.len())
            .map_err(|_| Error::WriteTooLarge("write byte count overflowed"))?;
        if len > MAX_WRITE_BYTES {
            return Err(Error::WriteTooLarge("write exceeds the 8 MiB per-call cap"));
        }
        let already_written = self.written_bytes.get();
        let next_total = already_written
            .checked_add(len)
            .ok_or(Error::WriteTooLarge("write byte count overflowed"))?;
        if next_total > self.write_max_bytes {
            return Err(Error::WriteTooLarge(
                "write exceeds files.write_max_bytes for this request",
            ));
        }
        // A request that already exceeded its script deadline must not create
        // or replace anything: the client stopped waiting, and a late write
        // would be an unobservable side effect.
        if Instant::now() >= self.write_deadline {
            return Err(Error::Io("write deadline exceeded".to_string()));
        }

        let (target, parent) = self.resolve_for_write(path)?;
        atomic_replace(&target, &parent, bytes, self.write_deadline)?;
        self.written_bytes.set(next_total);
        Ok(len)
    }

    /// Resolve a write target without following symlinks. The final component
    /// may be absent; every existing parent must be a real directory, and an
    /// existing final target must be a regular file.
    fn resolve_for_write(&self, raw: &str) -> Result<(PathBuf, PathBuf), Error> {
        if raw.is_empty() {
            return Err(Error::PathInvalid("path must not be empty"));
        }
        let windows_separator = cfg!(windows) && (raw.ends_with('\\') || raw.ends_with("\\."));
        if raw.ends_with('/') || raw.ends_with("/.") || windows_separator {
            return Err(Error::PathInvalid("path must name a file"));
        }
        let mut parts = Vec::new();
        for component in Path::new(raw).components() {
            match component {
                Component::ParentDir => {
                    return Err(Error::PathInvalid("`..` components are not allowed"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(Error::PathInvalid("absolute paths are not allowed"));
                }
                Component::CurDir => {}
                Component::Normal(part) => {
                    #[cfg(windows)]
                    validate_write_component(part)?;
                    parts.push(part.to_os_string());
                }
            }
        }
        let Some(file_name) = parts.pop() else {
            return Err(Error::PathInvalid("path must name a file"));
        };

        let root =
            std::fs::canonicalize(&self.root).map_err(|error| Error::Io(error.to_string()))?;
        if !root.is_dir() {
            return Err(Error::Io("file root is not a directory".to_string()));
        }
        let mut parent = root.clone();
        for part in &parts {
            parent.push(part);
            let metadata = std::fs::symlink_metadata(&parent).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::Io("parent directory does not exist".to_string())
                } else {
                    Error::Io(error.to_string())
                }
            })?;
            if is_link_like(&metadata) {
                return Err(Error::PathInvalid(
                    "symbolic links are not allowed for writes",
                ));
            }
            if !metadata.is_dir() {
                return Err(Error::Io("parent path is not a directory".to_string()));
            }
        }
        let parent =
            std::fs::canonicalize(&parent).map_err(|error| Error::Io(error.to_string()))?;
        if !parent.starts_with(&root) {
            return Err(Error::PathInvalid("path escapes the file root"));
        }
        let target = parent.join(file_name);
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if is_link_like(&metadata) {
                    return Err(Error::PathInvalid(
                        "symbolic links are not allowed for writes",
                    ));
                }
                if !metadata.is_file() {
                    return Err(Error::Io("path is not a regular file".to_string()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Io(error.to_string())),
        }
        Ok((target, parent))
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
            return Err(Error::Io("file root is not a directory".to_string()));
        }
        let target = std::fs::canonicalize(root.join(raw)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound
            } else {
                Error::Io(error.to_string())
            }
        })?;
        if !target.starts_with(&root) {
            return Err(Error::PathInvalid("path escapes the file root"));
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
            Err(Error::PathInvalid("path escapes the file root"))
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn verify_open_target(&self, _file: &File, _root: &Path) -> Result<(), Error> {
        Ok(())
    }
}

fn atomic_replace(
    target: &Path,
    parent: &Path,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), Error> {
    let mut temp = tempfile::Builder::new()
        .prefix(".stuntdouble-write-")
        .tempfile_in(parent)
        .map_err(|error| Error::Io(error.to_string()))?;
    temp.write_all(bytes)
        .map_err(|error| Error::Io(error.to_string()))?;
    temp.as_file_mut()
        .flush()
        .map_err(|error| Error::Io(error.to_string()))?;
    persist_replacement(temp, target, deadline)
}

/// Replace `target` with an already-written temporary file.
///
/// On Windows, concurrent `MoveFileExW` calls over the same target can fail
/// with `ERROR_ACCESS_DENIED`. All script writes in one `serve` process go
/// through this host, so a process-local lock serializes the replace step and
/// lets the documented last-writer-wins behavior hold. The lock is an
/// implementation detail, not a user-facing lock/queue; separate `serve`
/// processes remain uncoordinated, as ADR 0022 specifies. The retry covers
/// transient external holders without widening the stable error code.
fn persist_replacement(
    mut temp: tempfile::NamedTempFile,
    target: &Path,
    deadline: Instant,
) -> Result<(), Error> {
    let mut last_error = None;
    for attempt in 0..PERSIST_ATTEMPTS {
        // The caller's script deadline bounds every retry. Stop before the
        // next attempt or backoff window once it has passed.
        if Instant::now() >= deadline {
            return Err(Error::Io("write deadline exceeded".to_string()));
        }
        let result = {
            #[cfg(windows)]
            let _guard = replace_lock();
            temp.persist(target)
        };
        match result {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = Some(error.error);
                temp = error.file;
                if attempt + 1 < PERSIST_ATTEMPTS {
                    let backoff = std::time::Duration::from_millis(10 * (attempt as u64 + 1));
                    if Instant::now() + backoff >= deadline {
                        return Err(Error::Io("write deadline exceeded".to_string()));
                    }
                    std::thread::sleep(backoff);
                }
            }
        }
    }
    Err(Error::Io(last_error.map_or_else(
        || "failed to replace the target file".to_string(),
        |error| error.to_string(),
    )))
}

#[cfg(windows)]
fn replace_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(windows)]
const PERSIST_ATTEMPTS: usize = 3;

#[cfg(not(windows))]
const PERSIST_ATTEMPTS: usize = 2;

fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt as _;

        let file_type = metadata.file_type();
        if file_type.is_symlink_dir() || file_type.is_symlink_file() {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn validate_write_component(part: &std::ffi::OsStr) -> Result<(), Error> {
    const RESERVED: [&str; 25] = [
        "CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4",
        "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
        "LPT7", "LPT8", "LPT9",
    ];
    const RESERVED_SUPERSCRIPT: [&str; 6] = ["COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³"];

    let Some(name) = part.to_str() else {
        return Err(Error::PathInvalid(
            "path components must be valid Unicode on Windows",
        ));
    };
    let reserved = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let invalid_character = name.chars().any(|character| {
        character < '\u{20}' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*' | ':')
    });
    if invalid_character
        || name.ends_with('.')
        || name.ends_with(' ')
        || RESERVED.contains(&reserved.as_str())
        || RESERVED_SUPERSCRIPT.contains(&reserved.as_str())
    {
        return Err(Error::PathInvalid(
            "path component is reserved or invalid on Windows",
        ));
    }
    Ok(())
}

fn map_open_error(error: &std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::NotFound
    } else {
        Error::Io(error.to_string())
    }
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
pub type CallHandle = Pending<CallRecord>;

impl Pending<CallRecord> {
    /// Finalize this call with the outcome of its body stream.
    pub fn finish(&self, outcome: StreamOutcome, bytes: u64) {
        self.finish_with(bytes, |record| {
            record.error = outcome.error_class();
        });
    }
}

#[cfg(test)]
fn error_json(error: &Error) -> Json {
    json!({
        "ok": false,
        "code": error.code(),
        "message": error.message(),
    })
}

#[cfg(test)]
fn reply_json(reply: FileReply) -> Json {
    match reply {
        FileReply::Text(text) => json!({ "ok": true, "text": text }),
        FileReply::Bytes(bytes) => {
            json!({ "ok": true, "bytes_base64": BASE64.encode(bytes) })
        }
        FileReply::Stream(handle) => json!({ "ok": true, "handle": handle }),
        FileReply::Written(bytes) => json!({ "ok": true, "bytes": bytes }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_access(root: &Path, writable: bool, write_max_bytes: u64) -> (FileAccess, CallLog) {
        let calls = CallLog::new();
        (
            FileAccess::new(
                root,
                writable,
                write_max_bytes,
                Instant::now() + std::time::Duration::from_secs(30),
                calls.clone(),
            ),
            calls,
        )
    }

    #[test]
    fn script_file_writes_are_disabled_by_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, calls) = write_access(dir.path(), false, 1024);
        let result = access.call(&json!({
            "op": "writeText",
            "path": "created.txt",
            "text": "hello"
        }));
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "file_write_disabled");
        assert!(!dir.path().join("created.txt").exists());
        let calls = calls.snapshot();
        assert_eq!(calls[0]["api"], "file.writeText");
        assert_eq!(calls[0]["error"], "file_write_disabled");
    }

    #[test]
    fn script_write_text_creates_replaces_and_reads_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, calls) = write_access(dir.path(), true, 1024);
        let first = access.call(&json!({
            "op": "writeText",
            "path": "nested/report.txt",
            "text": "old"
        }));
        assert_eq!(first["ok"], false);
        assert_eq!(first["code"], "file_io_error");

        std::fs::create_dir(dir.path().join("nested")).expect("nested dir");
        let first = access.call(&json!({
            "op": "writeText",
            "path": "nested/report.txt",
            "text": "old"
        }));
        assert_eq!(first["ok"], true);
        assert_eq!(first["bytes"], 3);

        let second = access.call(&json!({
            "op": "writeText",
            "path": "nested/report.txt",
            "text": "new value"
        }));
        assert_eq!(second["ok"], true);
        assert_eq!(second["bytes"], 9);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/report.txt")).expect("written file"),
            "new value"
        );

        let read = access.call(&json!({ "op": "readText", "path": "nested/report.txt" }));
        assert_eq!(read["ok"], true);
        assert_eq!(read["text"], "new value");
        let calls = calls.snapshot();
        assert_eq!(calls[0]["error"], "file_io_error");
        assert_eq!(calls[1]["api"], "file.writeText");
        assert_eq!(calls[1]["bytes"], 3);
        assert_eq!(calls[2]["bytes"], 9);
    }

    #[test]
    fn script_write_bytes_round_trips_binary_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, _) = write_access(dir.path(), true, 1024);
        let bytes = vec![0_u8, 1, 127, 128, 255];
        let result = access.call(&json!({
            "op": "writeBytes",
            "path": "binary.bin",
            "bytes_base64": BASE64.encode(&bytes)
        }));
        assert_eq!(result["ok"], true);
        assert_eq!(result["bytes"], 5);
        assert_eq!(
            std::fs::read(dir.path().join("binary.bin")).expect("written file"),
            bytes
        );
    }

    #[test]
    fn script_write_rejects_empty_and_dot_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, _) = write_access(dir.path(), true, 1024);
        for path in ["", ".", "./", "sub/.."] {
            let result = access.call(&json!({
                "op": "writeText",
                "path": path,
                "text": "x"
            }));
            assert_eq!(result["ok"], false, "path: {path}");
            assert_eq!(result["code"], "file_path_invalid", "path: {path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn script_write_replace_breaks_hard_link_outside_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(dir.path().join("linked.txt"), "old").expect("target file");
        std::fs::hard_link(
            dir.path().join("linked.txt"),
            outside.path().join("linked.txt"),
        )
        .expect("hard link");
        let (access, _) = write_access(dir.path(), true, 1024);
        let result = access.call(&json!({
            "op": "writeText",
            "path": "linked.txt",
            "text": "new"
        }));
        assert_eq!(result["ok"], true, "result: {result}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("linked.txt")).expect("target file"),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join("linked.txt")).expect("outside file"),
            "old",
            "rename replacement must not mutate the outside inode"
        );
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_readers_see_a_complete_version_during_replace() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        const READERS: usize = 4;
        const WRITES: usize = 20;
        // Every reader must still be inside its read loop when the writes run,
        // and must complete several loop reads; otherwise the assertions only
        // cover the pre-barrier read and the round proves nothing.
        const MIN_READS_PER_THREAD: usize = 4;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = Arc::new(dir.path().to_path_buf());
        let old_payload = "old-".repeat(1024).into_bytes();
        let new_payload = "new-".repeat(1024).into_bytes();
        std::fs::write(root.join("concurrent.bin"), &old_payload).expect("seed file");

        let stopping = Arc::new(AtomicBool::new(false));
        let started = Arc::new(std::sync::Barrier::new(READERS + 1));
        let readers_looping = Arc::new(AtomicUsize::new(0));
        let handles = (0..READERS)
            .map(|_| {
                let root = Arc::clone(&root);
                let stopping = Arc::clone(&stopping);
                let started = Arc::clone(&started);
                let readers_looping = Arc::clone(&readers_looping);
                let old_payload = old_payload.clone();
                let new_payload = new_payload.clone();
                std::thread::spawn(move || {
                    let (access, _) = write_access(root.as_path(), true, 1024 * 1024);
                    let old_text = String::from_utf8_lossy(&old_payload).into_owned();
                    let new_text = String::from_utf8_lossy(&new_payload).into_owned();
                    let mut reads = 0_usize;

                    // Prove this reader is live before the writer starts, so a
                    // busy runner cannot stop the writer before any read runs.
                    let result = access.call(&json!({
                        "op": "readText",
                        "path": "concurrent.bin"
                    }));
                    let text = result["text"].as_str().expect("read text");
                    assert!(
                        text == old_text || text == new_text,
                        "reader saw an incomplete version"
                    );
                    reads += 1;
                    started.wait();
                    readers_looping.fetch_add(1, Ordering::Release);

                    while reads < MIN_READS_PER_THREAD || !stopping.load(Ordering::Relaxed) {
                        let result = access.call(&json!({
                            "op": "readText",
                            "path": "concurrent.bin"
                        }));
                        let text = result["text"].as_str().expect("read text");
                        assert!(
                            text == old_text || text == new_text,
                            "reader saw an incomplete version"
                        );
                        reads += 1;
                    }
                    reads
                })
            })
            .collect::<Vec<_>>();

        let (writer_access, _) = write_access(root.as_path(), true, 1024 * 1024);
        started.wait();
        while readers_looping.load(Ordering::Acquire) < READERS {
            std::thread::yield_now();
        }
        for _ in 0..WRITES {
            let result = writer_access.call(&json!({
                "op": "writeBytes",
                "path": "concurrent.bin",
                "bytes_base64": BASE64.encode(&new_payload)
            }));
            assert_eq!(result["ok"], true, "result: {result}");
        }
        stopping.store(true, Ordering::Relaxed);
        for handle in handles {
            let reads = handle.join().expect("reader");
            assert!(
                reads >= MIN_READS_PER_THREAD,
                "reader only completed {reads} reads"
            );
        }
    }

    #[test]
    fn script_write_budget_is_per_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, _) = write_access(dir.path(), true, 4);
        let first = access.call(&json!({
            "op": "writeText",
            "path": "first.txt",
            "text": "four"
        }));
        assert_eq!(first["ok"], true);
        let second = access.call(&json!({
            "op": "writeText",
            "path": "second.txt",
            "text": "x"
        }));
        assert_eq!(second["ok"], false);
        assert_eq!(second["code"], "file_too_large");
        assert!(!dir.path().join("second.txt").exists());
    }

    #[test]
    fn script_write_past_the_deadline_leaves_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("existing.txt"), "old").expect("old file");
        let calls = CallLog::new();
        let expired = Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("deadline before now");
        let (access, _) = (
            FileAccess::new(dir.path(), true, 1024, expired, calls.clone()),
            calls,
        );

        let created = access.call(&json!({
            "op": "writeText",
            "path": "created.txt",
            "text": "x"
        }));
        assert_eq!(created["ok"], false);
        assert_eq!(created["code"], "file_io_error");
        assert!(!dir.path().join("created.txt").exists());

        let replaced = access.call(&json!({
            "op": "writeText",
            "path": "existing.txt",
            "text": "new"
        }));
        assert_eq!(replaced["ok"], false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("existing.txt")).expect("old file"),
            "old"
        );
    }

    #[test]
    fn script_write_rejects_oversized_input_before_touching_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("too-large.txt"), "old").expect("old file");
        let (access, _) = write_access(dir.path(), true, u64::MAX);
        let text = "x".repeat(usize::try_from(MAX_WRITE_BYTES).expect("cap") + 1);
        let result = access.call(&json!({
            "op": "writeText",
            "path": "too-large.txt",
            "text": text
        }));
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "file_too_large");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("too-large.txt")).expect("old file"),
            "old"
        );
    }

    #[test]
    fn script_write_rejects_directories_and_missing_parents() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("directory")).expect("directory");
        let (access, _) = write_access(dir.path(), true, 1024);
        let directory = access.call(&json!({
            "op": "writeText",
            "path": "directory",
            "text": "x"
        }));
        assert_eq!(directory["code"], "file_io_error");
        let missing = access.call(&json!({
            "op": "writeText",
            "path": "missing/file.txt",
            "text": "x"
        }));
        assert_eq!(missing["code"], "file_io_error");
        let trailing_separator = access.call(&json!({
            "op": "writeText",
            "path": "trailing/",
            "text": "x"
        }));
        assert_eq!(trailing_separator["code"], "file_path_invalid");
        assert!(!dir.path().join("trailing").exists());
        let trailing_dot = access.call(&json!({
            "op": "writeText",
            "path": "trailing/.",
            "text": "x"
        }));
        assert_eq!(trailing_dot["code"], "file_path_invalid");
        assert!(!dir.path().join("trailing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn script_write_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("secret.txt"), "secret").expect("outside file");
        symlink(outside.path(), dir.path().join("outside-dir")).expect("dir symlink");
        symlink(
            outside.path().join("secret.txt"),
            dir.path().join("outside-file.txt"),
        )
        .expect("file symlink");
        let (access, _) = write_access(dir.path(), true, 1024);
        let parent = access.call(&json!({
            "op": "writeText",
            "path": "outside-dir/escape.txt",
            "text": "overwritten"
        }));
        assert_eq!(parent["code"], "file_path_invalid");
        let final_target = access.call(&json!({
            "op": "writeText",
            "path": "outside-file.txt",
            "text": "overwritten"
        }));
        assert_eq!(final_target["code"], "file_path_invalid");
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).expect("outside file"),
            "secret"
        );
    }

    #[test]
    fn concurrent_writes_leave_one_complete_version() {
        const WRITERS: usize = 8;
        const ROUNDS: usize = 3;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = Arc::new(dir.path().to_path_buf());
        let payloads = (0..WRITERS)
            .map(|index| format!("version-{index}-").repeat(1024).into_bytes())
            .collect::<Vec<_>>();

        // Repeat rounds so a regression on the Windows replace path is not
        // masked by scheduling.
        for _ in 0..ROUNDS {
            let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
            let handles = payloads
                .iter()
                .cloned()
                .map(|payload| {
                    let root = Arc::clone(&root);
                    let barrier = Arc::clone(&barrier);
                    let encoded = BASE64.encode(&payload);
                    std::thread::spawn(move || {
                        let (access, _) = write_access(root.as_path(), true, 1024 * 1024);
                        barrier.wait();
                        let result = access.call(&json!({
                            "op": "writeBytes",
                            "path": "concurrent.bin",
                            "bytes_base64": encoded
                        }));
                        (result, payload)
                    })
                })
                .collect::<Vec<_>>();

            let results = handles
                .into_iter()
                .map(|handle| handle.join().expect("writer"))
                .collect::<Vec<_>>();
            for (result, _) in &results {
                assert_eq!(result["ok"], true, "result: {result}");
            }
            let final_bytes =
                std::fs::read(dir.path().join("concurrent.bin")).expect("written file");
            assert!(
                results.iter().any(|(_, payload)| final_bytes == *payload),
                "final file is not one complete version"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn script_write_rejects_windows_alternate_data_streams_and_reserved_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, _) = write_access(dir.path(), true, 1024);
        for path in [
            "report.txt:stream",
            "CON",
            "nul.txt",
            "COM1",
            "LPT².log",
            "trailing.",
            "trailing ",
            "bad?.txt",
        ] {
            let result = access.call(&json!({
                "op": "writeText",
                "path": path,
                "text": "x"
            }));
            assert_eq!(result["code"], "file_path_invalid", "path: {path}");
        }
        assert!(!dir.path().join("report.txt").exists());
    }

    #[test]
    fn failed_file_stream_records_one_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (access, calls) = write_access(dir.path(), false, 1024);

        let result = access.call(&json!({ "op": "stream", "path": "missing.bin" }));
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "file_not_found");
        let calls = calls.snapshot();
        assert_eq!(calls.len(), 1, "calls: {calls:?}");
        assert_eq!(calls[0]["api"], "file.stream");
        assert_eq!(calls[0]["error"], "file_not_found");
    }

    #[test]
    fn failed_upload_stream_records_a_stable_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("0");
        std::fs::write(&path, b"data").expect("upload fixture");
        let store = Arc::new(UploadStore {
            dir: Mutex::new(Some(temp)),
            files: vec![UploadedFile {
                meta: UploadFileMeta {
                    field: "document".to_string(),
                    filename: "fixture.bin".to_string(),
                    content_type: None,
                    size: 4,
                },
                path,
            }],
            total_bytes: 4,
        });
        let calls: CallLog = Chain::new();
        let access = UploadAccess::new(Arc::clone(&store), calls.clone());
        store.close();

        let result = access.call(&json!({ "op": "stream", "index": 0 }));
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "file_not_found");
        let calls = calls.snapshot();
        assert_eq!(calls.len(), 1, "calls: {calls:?}");
        assert_eq!(calls[0]["api"], "upload.stream");
        assert_eq!(calls[0]["error"], "file_not_found");
        assert!(calls[0]["duration_ms"].is_number(), "calls: {calls:?}");
    }
}
