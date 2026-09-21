# Stunt Double

Stunt Double is a mock server for integration testing against real external dependencies. This glossary defines the project language. It contains no implementation details.

## Language

**Route**:
A declared mock endpoint with a match condition and one response pipeline.
_Avoid_: endpoint definition, handler, stub

**Match**:
The part of a route that decides whether an incoming request belongs to it.
_Avoid_: filter, predicate, selector

**Source**:
The origin of data used to build a response, such as a static file or an upstream HTTP call.
_Avoid_: data provider, backend, fetcher

**Transform**:
The script step that turns source data into response data.
_Avoid_: mapper, converter, processor

**Response**:
The status, headers, and body returned to the client for a matched route.
_Avoid_: reply, output, result

**Host function**:
A function injected by the Rust host into the script runtime. All external script capabilities are host functions.
_Avoid_: native function, binding, built-in

**Script sandbox**:
The set of limits and capabilities that constrain script execution.
_Avoid_: jail, container, isolation layer

**Static file root**:
The single configured directory from which scripts may read files.
_Avoid_: file root, document root, upload directory

**Upstream failure**:
A failure to obtain an HTTP response from an upstream call. A non-2xx HTTP response is data, not an upstream failure.
Exception: a streaming capability that hands the body to the client instead of the script cannot treat a final non-2xx response as data; it surfaces that response as a catchable error so the script can still own the client-visible status.
_Avoid_: upstream error, backend error

**Request-local state**:
Data stored during one request and destroyed when that request ends.
_Avoid_: session, cache, shared state

**Shared state**:
Data that persists or is visible across requests. Stunt Double v1 does not provide it.
_Avoid_: global state, cross-request state

**Route model**:
A configuration model where each interface is declared explicitly.
_Avoid_: resource model, auto CRUD

**Resource model**:
A configuration model where REST routes are derived from the shape of a data file.
_Avoid_: route model, json-server mode

**Preset generator**:
A tool that compiles a higher-level input, such as an OpenAPI document, into ordinary routes.
_Avoid_: resource engine, second engine
