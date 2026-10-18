//! Stunt Double library: configuration, matching, script execution, HTTP server.
//! Contracts: docs/contracts/
pub mod config;
mod files;
pub mod matcher;
mod request_log;
pub mod script;
pub mod server;
mod upstream;

// ADR 0014 (revised in #102): the crate denies `unsafe_code`; this module is
// the single reviewed exception (Job Object FFI, see src/windows_job.rs).
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_job;
