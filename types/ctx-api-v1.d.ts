// Type definitions for the Stunt Double host-injected `ctx` object, apiVersion 1.
//
// This file is part of the public contract; the release artifact set defined by
// plans/adr/0012-release-artifacts-and-supply-chain.md must include it. T9 wires
// the release pipeline.
// It covers the first-slice subset implemented by T1-T8; pending capabilities
// are intentionally absent until they land. Keep this file in sync with
// docs/contracts/ctx-api.md.

/** A read-only snapshot of the incoming client request. */
interface SdRequest {
  readonly method: string;
  readonly path: string;
  readonly params: Readonly<Record<string, string>>;
  /** Percent-decoded query names and values; a repeated name keeps the last value. */
  readonly query: Readonly<Record<string, string>>;
  /** Lowercased header names; a repeated name keeps the last value. */
  readonly headers: Readonly<Record<string, string>>;
  /** `null` when the request body is not valid UTF-8. */
  readonly bodyText: string | null;
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

type SdErrorCode =
  | "script_error"
  | "upstream_url_invalid"
  | "upstream_redirect_error"
  | "upstream_unreachable"
  | "upstream_http_error";

/** Host-created errors carry a stable `code`; ordinary script errors may omit it. */
interface SdError extends Error {
  readonly code?: SdErrorCode;
}

/** Host-injected global available in every Route script. */
interface SdContext {
  readonly apiVersion: "1";
  readonly request: SdRequest;
  readonly http: SdHttp;
  readonly env: Readonly<Record<string, string>>;
  readonly log: SdLog;
  /**
   * Produces the client Response. The first call wins and returns true;
   * later calls are ignored and return false. Bytes must be integers in [0, 255].
   * Malformed header names or values throw a catchable `script_error` and do
   * not record a Response.
   */
  respond(
    status: number,
    headers?: SdHeaders,
    body?: string | Uint8Array | ArrayBuffer | readonly number[],
  ): boolean;
}

/** Host-injected global available in every Route script. */
declare const ctx: SdContext;
