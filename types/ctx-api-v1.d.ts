// Type definitions for the Stunt Double host-injected `ctx` object, apiVersion 1.
//
// This file is part of the public contract; the release artifact set defined by
// plans/adr/0012-release-artifacts-and-supply-chain.md must include it. T9 wires
// the release pipeline.
// It covers the implemented subset through T1-T13 plus the v2 process-local
// Shared state slice and host crypto API (ADR 0019); capabilities deferred by
// plans/adr/0016-v1-ctx-pending-capabilities-deferred.md are intentionally
// absent. Keep this file in sync with docs/contracts/ctx-api.md.

/** A read-only snapshot of the incoming client request. */
interface SdRequest {
  readonly method: string;
  readonly path: string;
  readonly params: Readonly<Record<string, string>>;
  /** Percent-decoded query names and values; a repeated name keeps the last value. */
  readonly query: Readonly<Record<string, string>>;
  /**
   * The raw query component of the request line, exactly as sent: duplicate
   * names, order, and percent-encoding are preserved. `""` when the request
   * has no query. Use with `query` for signature canonicalization.
   */
  readonly rawQuery: string;
  /** Lowercased header names; a repeated name keeps the last value. */
  readonly headers: Readonly<Record<string, string>>;
  /**
   * `null` when the request body is not valid UTF-8, and for multipart
   * requests because the framing is consumed before the script runs.
   */
  readonly bodyText: string | null;
  /**
   * The exact request body bytes. `null` when the body was not read
   * (multipart requests, where the framing is consumed before the script
   * runs); an empty `Uint8Array` means a read empty body. No UTF-8
   * conversion is applied.
   */
  readonly bodyBytes: Uint8Array | null;
  /** Uploaded files in multipart order; empty for non-multipart requests. */
  readonly files: readonly SdUploadedFile[];
}

/** One uploaded file exposed through `ctx.request.files`. */
interface SdUploadedFile {
  readonly field: string;
  /** Client basename after `/` and `\` path components are stripped. */
  readonly filename: string;
  readonly contentType: string | null;
  /** File size in bytes. */
  readonly size: number;
  /**
   * Reads the upload as strict UTF-8. Throws a catchable error with code
   * "file_too_large", "file_encoding_error", or "file_io_error".
   */
  text(): string;
  /**
   * Reads the upload as bytes. Throws a catchable error with code
   * "file_too_large" or "file_io_error".
   */
  bytes(): Uint8Array;
  /**
   * Opens the upload as an opaque, single-consumption streamed Response body.
   * Valid only as the `body` argument of `ctx.respond` in the same request.
   */
  stream(): SdFileStream;
}

/** An allowlisted upstream HTTP response returned by `ctx.http.get`. */
interface SdUpstreamResponse {
  readonly status: number;
  readonly headers: Readonly<Record<string, string>>;
  /** Decodes the body as UTF-8, replacing invalid sequences. */
  text(): string;
  /** Copies the body into a new Uint8Array. */
  bytes(): Uint8Array;
}

interface SdHttpGetOptions {
  /**
   * Per-call timeout upper bound in milliseconds. Must be a positive integer;
   * the remaining script budget can shorten the effective timeout.
   */
  timeout_ms?: number;
}

interface SdHttpPipeOptions {
  /**
   * Client Response status; defaults to the upstream 2xx status (for example
   * 200, or 206 for a partial Response). Must be an integer in [100, 599].
   */
  status?: number;
  headers?: SdHeaders;
}

interface SdHttp {
  get(url: string, opts?: SdHttpGetOptions): SdUpstreamResponse;
  /**
   * Streams the upstream body to the client without entering the script heap.
   * The client Range header is forwarded, and an upstream 206 keeps its
   * Content-Range header. Returns true when it produced the Response, or
   * false when an earlier Response already won. A final non-2xx upstream
   * answer throws a catchable error with code "upstream_http_error".
   * Malformed header names or values throw `script_error` before the upstream
   * call starts.
   */
  pipe(url: string, opts?: SdHttpPipeOptions): boolean;
}

declare const sdFileStreamBrand: unique symbol;

/**
 * Opaque, single-consumption handle returned by `ctx.file.stream` or
 * `ctx.request.files[].stream()`. It exposes
 * no readable properties and is valid only as the `body` argument of
 * `ctx.respond` in the same request. The brand symbol makes the handle
 * impossible to construct outside the host.
 */
interface SdFileStream {
  /** @internal */
  readonly [sdFileStreamBrand]: true;
}

interface SdFile {
  /**
   * Reads one file inside the static file root as strict UTF-8. Throws a
   * catchable error with code "file_path_invalid", "file_not_found",
   * "file_too_large", "file_encoding_error", or "file_io_error".
   */
  readText(path: string): string;
  /**
   * Reads one file inside the static file root as bytes. Throws a catchable
   * error with code "file_path_invalid", "file_not_found", "file_too_large",
   * or "file_io_error".
   */
  readBytes(path: string): Uint8Array;
  /** Opens one file inside the static file root as a streamed Response body. */
  stream(path: string): SdFileStream;
}

type SdHeaderValue = string | number | boolean | readonly string[];

interface SdRespondHeaders {
  readonly [name: string]: SdHeaderValue;
}

type SdHeaderPairs = ReadonlyArray<readonly [string, string]>;

/**
 * Header object or [name, value] pairs. The host validates names and values;
 * malformed pairs throw a catchable `script_error`.
 */
type SdHeaders = SdRespondHeaders | SdHeaderPairs;

interface SdLog {
  info(...values: unknown[]): void;
  warn(...values: unknown[]): void;
  error(...values: unknown[]): void;
}

interface SdStateObject {
  readonly [key: string]: SdStateValue;
}

/** A losslessly JSON-serializable value accepted by `ctx.state`. */
type SdStateValue =
  | null
  | boolean
  | number
  | string
  | readonly SdStateValue[]
  | SdStateObject;

interface SdStateOptions {
  /**
   * Per-key lifetime in milliseconds. Must be a positive safe integer in
   * [1, 604800000] (7 days). TTL starts when a mutation commits and does not
   * slide on reads.
   */
  ttl_ms?: number;
}

/**
 * Process-local state shared by all Route scripts in one running `serve`
 * instance. The store starts empty and is cleared by restart.
 */
interface SdState {
  /** Returns an independent copy, or `undefined` when absent or expired. */
  get(key: string): SdStateValue | undefined;
  /** Inserts or replaces a value and resets its TTL. */
  set(key: string, value: SdStateValue, opts?: SdStateOptions): void;
  /** Atomically returns an independent copy and removes the key. */
  take(key: string): SdStateValue | undefined;
  /**
   * Atomically stores the value only when the key is absent or expired.
   * Returns true when stored and false when a live value already exists.
   */
  setIfAbsent(key: string, value: SdStateValue, opts?: SdStateOptions): boolean;
}

/** Input accepted by `ctx.crypto` hashing, HMAC, and constant-time comparison. */
type SdCryptoInput = string | Uint8Array | ArrayBuffer | readonly number[];

/**
 * Host-provided cryptographic primitives. All operations run in the Rust
 * host; inputs and outputs are never shared across calls.
 */
interface SdCrypto {
  /**
   * Reads cryptographically secure random bytes from the operating system CSPRNG.
   * Throws `crypto_invalid_input` when `byteLength` is not an integer in [1, 1024].
   * Throws `crypto_random_unavailable` when the system entropy source fails.
   */
  randomBytes(byteLength: number): Uint8Array;
  /**
   * Computes the SHA-256 digest of `input`.
   * Throws `crypto_invalid_input` when input exceeds 8 MiB or contains invalid types,
   * out-of-range byte values, or lone UTF-16 surrogates.
   */
  sha256(input: SdCryptoInput): Uint8Array;
  /**
   * Computes the HMAC-SHA256 signature for `message` with `key`.
   * Throws `crypto_invalid_input` when `key` exceeds 1 KiB, `message` exceeds 8 MiB,
   * or an argument contains invalid types, out-of-range byte values, or lone surrogates.
   */
  hmacSha256(key: SdCryptoInput, message: SdCryptoInput): Uint8Array;
  /**
   * Compares two byte sequences in constant time.
   * Each argument must be in [1, 1024] bytes; throws `crypto_invalid_input` if out of bounds.
   * Returns false without short-circuiting when lengths differ.
   */
  timingSafeEqual(left: SdCryptoInput, right: SdCryptoInput): boolean;
}

type SdErrorCode =
  | "script_error"
  | "upstream_url_invalid"
  | "upstream_redirect_error"
  | "upstream_unreachable"
  | "upstream_http_error"
  | "file_path_invalid"
  | "file_not_found"
  | "file_too_large"
  | "file_encoding_error"
  | "file_io_error"
  | "state_invalid_input"
  | "state_capacity_exceeded"
  | "state_internal_error"
  | "crypto_invalid_input"
  | "crypto_random_unavailable";

/** Host-created errors carry a stable `code`; ordinary script errors may omit it. */
interface SdError extends Error {
  readonly code?: SdErrorCode;
}

/** Host-injected global available in every Route script. */
interface SdContext {
  readonly apiVersion: "1";
  /** The match category for this request and the allowed methods, if any. */
  readonly match: SdMatch;
  readonly request: SdRequest;
  readonly http: SdHttp;
  readonly file: SdFile;
  readonly env: Readonly<Record<string, string>>;
  readonly log: SdLog;
  readonly state: SdState;
  readonly crypto: SdCrypto;
  /**
   * Produces the client Response. The first call wins and returns true;
   * later calls are ignored and return false. Bytes must be integers in [0, 255].
   * Malformed header names or values throw a catchable `script_error` and do
   * not record a Response.
   */
  respond(
    status: number,
    headers?: SdHeaders,
    body?: string | Uint8Array | ArrayBuffer | readonly number[] | SdFileStream,
  ): boolean;
}

/** Host-injected global available in every Route script. */
declare const ctx: SdContext;

/** The match category of a request, relative to the configured Routes. */
type SdMatchKind = "route" | "not_found" | "method_not_allowed";

/**
 * The match category for a request and the allowed methods, if any.
 * A `method_not_allowed` fallback must respond with 405 and must not set
 * `Allow`; the host injects it from `allowedMethods`.
 */
interface SdMatch {
  readonly kind: SdMatchKind;
  /** Empty unless `kind` is `"method_not_allowed"`; then uppercase, deduplicated methods in first-declaration order. */
  readonly allowedMethods: readonly string[];
}
