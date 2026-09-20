// Type definitions for the Stunt Double `ctx` API, apiVersion 1.
// This file is part of the public contract: docs/contracts/ctx-api.md.
// It covers the T2-T4 subset; T8 (#11) publishes it with release artifacts
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

interface SdHttp {
  get(url: string, opts?: SdHttpGetOptions): SdUpstreamResponse;
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
  /** Host errors use "script_error" or "upstream_unreachable". */
  code?: string;
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
