// Type definitions for the Stunt Double `ctx` API, apiVersion 1.
// This file is part of the public contract: docs/contracts/ctx-api.md.
// It covers the T2-T6 subset; T8 (#11) publishes it with release artifacts
// and extends it as later capability groups land.

/** A read-only snapshot of the incoming client request. */
interface SdRequest {
  readonly method: string;
  readonly path: string;
  readonly params: Readonly<Record<string, string>>;
  readonly query: Readonly<Record<string, string>>;
  readonly headers: Readonly<Record<string, string>>;
  readonly bodyText: string | null;
}

/** An allowlisted upstream HTTP response. */
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
   * Client response status; defaults to the upstream 2xx status (for example
   * 200, or 206 for a partial response). Must be an integer in [100, 599].
   */
  status?: number;
  headers?: SdRespondHeaders | ReadonlyArray<readonly [string, string]>;
}

interface SdHttp {
  get(url: string, opts?: SdHttpGetOptions): SdUpstreamResponse;
  /**
   * Streams the upstream body to the client without entering the script heap.
   * The client Range header is forwarded, and an upstream 206 keeps its
   * Content-Range header. Returns true when it produced the response, or
   * false when an earlier response already won. A final non-2xx upstream
   * answer throws a catchable error with code "upstream_http_error".
   */
  pipe(url: string, opts?: SdHttpPipeOptions): boolean;
}

type SdHeaderValue = string | number | boolean | readonly string[];

interface SdRespondHeaders {
  readonly [name: string]: SdHeaderValue;
}

interface SdLog {
  info(...values: unknown[]): void;
  warn(...values: unknown[]): void;
  error(...values: unknown[]): void;
}

interface SdError extends Error {
  /**
   * Host errors use "script_error" (policy and validation failures),
   * "upstream_url_invalid" (ctx.http.pipe URL or scheme rejection),
   * "upstream_redirect_error" (an unfollowable ctx.http.pipe redirect chain),
   * "upstream_unreachable" (transport failures), or "upstream_http_error"
   * (a final non-2xx answer to ctx.http.pipe).
   */
  code?:
    | "script_error"
    | "upstream_url_invalid"
    | "upstream_redirect_error"
    | "upstream_unreachable"
    | "upstream_http_error";
}

interface SdContext {
  readonly apiVersion: "1";
  readonly request: SdRequest;
  readonly http: SdHttp;
  readonly env: Readonly<Record<string, string>>;
  readonly log: SdLog;
  respond(
    status: number,
    headers?: SdRespondHeaders | ReadonlyArray<readonly [string, string]>,
    body?: string | Uint8Array | ArrayBuffer | readonly number[],
  ): boolean;
}

declare const ctx: SdContext;
