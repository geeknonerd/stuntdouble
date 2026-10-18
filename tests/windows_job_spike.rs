#![cfg(windows)]
#![allow(unsafe_code)]

//! Throwaway spike for #102: prove the assumptions ADR 0014 D7 deferred.
//!
//! This is not production code and must not be merged. It drives the hidden
//! `__script-worker` entry point directly, wraps each worker in a per-worker
//! Job Object (`JOB_OBJECT_LIMIT_PROCESS_MEMORY` + `KILL_ON_JOB_CLOSE`), and
//! prints `SPIKE:` evidence lines for:
//!
//! 1. whether assigning a worker to an empty job works in the CI runner
//!    environment (nested-job question),
//! 2. what a plain script, an 8 MiB script, and a memory bomb do under a
//!    64 MiB commit cap (and how the failure is reported),
//! 3. whether a worker dies when its job-holding parent is hard-killed.
//!
//! Run: `cargo test --test windows_job_spike -- --nocapture --test-threads=1`

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use stuntdouble::script::WORKER_SUBCOMMAND;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const MIB: usize = 1024 * 1024;
/// Compiled-in floor the config layer enforces (`SCRIPT_MEMORY_LIMIT_FLOOR_MB`).
const FLOOR_MIB: usize = 64;

const OK_SCRIPT: &str = r#"ctx.respond(200, {}, "ok");"#;
const HEAVY_SCRIPT: &str = r#"
var buffers = [];
for (var i = 0; i < 8; i++) { buffers.push(new ArrayBuffer(1024 * 1024)); }
ctx.respond(200, {}, "allocated-8mib");
"#;
const BOMB_SCRIPT: &str = r#"
var buffers = [];
for (var i = 0; i < 128; i++) { buffers.push(new ArrayBuffer(1024 * 1024)); }
ctx.respond(200, {}, "allocated");
"#;
const FOREVER_SCRIPT: &str = r#"while (true) {}"#;

/// RAII wrapper around one Job Object handle.
struct Job(HANDLE);

impl Job {
    fn create(process_memory_bytes: usize) -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(format!("CreateJobObjectW error {}", GetLastError()));
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.ProcessMemoryLimit = process_memory_bytes;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let error = GetLastError();
                CloseHandle(handle);
                return Err(format!("SetInformationJobObject error {error}"));
            }
            Ok(Self(handle))
        }
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;

        let process = child.as_raw_handle();
        unsafe {
            if AssignProcessToJobObject(self.0, process) == 0 {
                Err(format!("AssignProcessToJobObject error {}", GetLastError()))
            } else {
                Ok(())
            }
        }
    }

    fn peak_process_commit(&self) -> Option<usize> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let mut returned = 0u32;
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut returned,
            )
        };
        (ok != 0).then_some(info.PeakProcessMemoryUsed)
    }

    /// Whether the test process itself already belongs to a job (runner probe).
    fn current_process_in_job() -> Option<bool> {
        let mut in_job = 0i32;
        let ok = unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) };
        (ok != 0).then_some(in_job != 0)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Debug)]
struct Run {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn job_line(script: &str) -> String {
    let request = json!({
        "method": "GET",
        "path": "/spike",
        "params": {},
        "query": {},
        "headers": {},
        "bodyText": null,
        "files": []
    });
    format!(
        "{}\n",
        json!({"type": "job", "script": script, "request": request})
    )
}

fn spawn_worker(memory_limit_mb: usize) -> Child {
    Command::new(env!("CARGO_BIN_EXE_stuntdouble"))
        .arg(WORKER_SUBCOMMAND)
        .arg("--memory-limit-mb")
        .arg(memory_limit_mb.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn worker")
}

fn drain(pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    rx
}

/// Feed one job to a fresh worker (optionally job-assigned) and collect the run.
///
/// The worker ping-pongs host calls (`__sd_validate_headers`) before it answers,
/// so this harness replies to every host call with `{"ok": true}` the same way
/// the real parent does for well-formed headers.
fn run_worker(job: Option<&Job>, script: &str, timeout: Duration) -> Run {
    let mut child = spawn_worker(FLOOR_MIB);
    if let Some(job) = job {
        match job.assign(&child) {
            Ok(()) => println!("SPIKE: assign=ok"),
            Err(error) => println!("SPIKE: assign=failed error={error}"),
        }
    }
    let mut stdin = std::io::BufWriter::new(child.stdin.take().expect("worker stdin"));
    stdin
        .write_all(job_line(script).as_bytes())
        .expect("write job line");
    stdin.flush().expect("flush job line");

    let stderr = drain(child.stderr.take().expect("worker stderr"));
    let stdout = child.stdout.take().expect("worker stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut collected = String::new();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            collected.push_str(&line);
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                break;
            };
            if value.get("type").and_then(|kind| kind.as_str()) != Some("host_call") {
                break;
            }
            let reply = json!({"type": "host_result", "result": {"ok": true}});
            let mut bytes = serde_json::to_vec(&reply).expect("reply json");
            bytes.push(b'\n');
            if stdin.write_all(&bytes).is_err() || stdin.flush().is_err() {
                break;
            }
        }
        let _ = tx.send(collected);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break Some(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    Run {
        timed_out: status.is_none(),
        status,
        stdout: rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default(),
        stderr: stderr
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_default(),
    }
}

fn report(case: &str, run: &Run, peak: Option<usize>) {
    println!(
        "SPIKE: case={case} exit={:?} timed_out={} peak_process_commit_mib={:?}",
        run.status,
        run.timed_out,
        peak.map(|bytes| bytes / MIB),
    );
    println!("SPIKE: case={case} stdout={:?}", run.stdout.trim());
    println!("SPIKE: case={case} stderr={:?}", run.stderr.trim());
}

fn process_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn job_assignment_and_plain_script_at_floor() {
    println!(
        "SPIKE: runner_process_in_job={:?}",
        Job::current_process_in_job()
    );
    let job = Job::create(FLOOR_MIB * MIB).expect("create job");
    let run = run_worker(Some(&job), OK_SCRIPT, Duration::from_secs(30));
    report("plain_at_floor", &run, job.peak_process_commit());
    assert!(
        run.status.is_some_and(|status| status.success()),
        "plain script did not finish cleanly: {run:?}"
    );
    assert!(
        run.stdout.contains("final_result"),
        "no final_result line: {}",
        run.stdout
    );
}

#[test]
fn plain_script_with_8mib_heap_at_floor() {
    let job = Job::create(FLOOR_MIB * MIB).expect("create job");
    let run = run_worker(Some(&job), HEAVY_SCRIPT, Duration::from_secs(30));
    report("heavy_8mib_at_floor", &run, job.peak_process_commit());
    assert!(
        run.status.is_some_and(|status| status.success()),
        "8 MiB script did not finish cleanly: {run:?}"
    );
    assert!(
        run.stdout.contains("allocated-8mib"),
        "8 MiB script did not respond: {}",
        run.stdout
    );
}

#[test]
fn memory_bomb_control_without_job() {
    let run = run_worker(None, BOMB_SCRIPT, Duration::from_secs(60));
    report("bomb_control_no_job", &run, None);
    assert!(
        run.status.is_some_and(|status| status.success()),
        "unlimited control run failed: {run:?}"
    );
}

#[test]
fn memory_bomb_inside_job_is_stopped() {
    let job = Job::create(FLOOR_MIB * MIB).expect("create job");
    let run = run_worker(Some(&job), BOMB_SCRIPT, Duration::from_secs(60));
    report("bomb_at_floor", &run, job.peak_process_commit());
    let classification = if run.stdout.contains("memory_limit") {
        "final_result_memory_limit"
    } else if run
        .stderr
        .lines()
        .any(|line| line.starts_with("memory allocation of "))
    {
        "allocator_abort_on_stderr"
    } else if !run.status.is_some_and(|status| status.success()) {
        "terminated_without_marker"
    } else {
        "succeeded"
    };
    println!("SPIKE: bomb_classification={classification}");
    assert_ne!(
        classification, "succeeded",
        "the job commit cap did not stop the bomb: {}",
        run.stdout
    );
}

#[test]
fn hard_kill_of_job_holder_terminates_worker() {
    let mut holder = Command::new(std::env::current_exe().expect("current exe"))
        .args(["--exact", "holder_mode", "--nocapture"])
        .env("SPIKE_HOLDER_MODE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn holder");

    let stdout = holder.stdout.take().expect("holder stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with("SPIKE-READY") {
                let _ = tx.send(line);
                return;
            }
        }
    });
    let ready = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("holder never reported ready");
    println!("SPIKE: holder_ready={ready}");
    let worker_pid: u32 = ready
        .split("pid=")
        .nth(1)
        .expect("pid in ready line")
        .trim()
        .parse()
        .expect("worker pid");

    assert!(
        process_alive(worker_pid),
        "worker {worker_pid} was not alive before the kill"
    );
    let kill = Command::new("taskkill")
        .args(["/F", "/PID", &holder.id().to_string()])
        .status()
        .expect("taskkill holder");
    assert!(kill.success(), "taskkill failed: {kill}");
    let _ = holder.wait();

    let start = Instant::now();
    let mut gone = false;
    while start.elapsed() < Duration::from_secs(5) {
        if !process_alive(worker_pid) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!(
        "SPIKE: kill_on_close worker_gone={gone} elapsed_ms={}",
        start.elapsed().as_millis()
    );
    assert!(
        gone,
        "worker {worker_pid} survived a hard kill of its job holder"
    );
}

/// Helper body: hold a job, park a long-running worker inside it, report the
/// worker pid, then sleep forever so the parent test can hard-kill us.
#[test]
fn holder_mode() {
    if std::env::var("SPIKE_HOLDER_MODE").is_err() {
        return;
    }
    let job = Job::create(256 * MIB).expect("create job");
    let mut worker = spawn_worker(256);
    job.assign(&worker).expect("assign worker");
    let mut stdin = worker.stdin.take().expect("worker stdin");
    stdin
        .write_all(job_line(FOREVER_SCRIPT).as_bytes())
        .expect("write job line");
    drop(stdin);
    // Give the worker time to enter the script loop before we report ready.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        worker.try_wait().expect("try_wait").is_none(),
        "worker exited before the kill"
    );
    println!("SPIKE-READY pid={}", worker.id());
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
