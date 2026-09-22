//! Command line surface for stuntdouble.
//! Contracts: docs/contracts/cli.md (commands, flags, exit codes)
//!   0 success, 1 runtime error, 2 configuration error, 3 internal error

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use stuntdouble::{config, server};

const DEFAULT_CONFIG: &str = "stuntdouble.toml";

#[derive(Parser)]
#[command(
    name = "stuntdouble",
    version,
    about = "Mock server for integration testing"
)]
struct Cli {
    /// Configuration file
    #[arg(short = 'c', long, global = true, default_value = DEFAULT_CONFIG)]
    config: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the mock server
    Serve {
        /// Attach a detail field to 502/500 responses
        #[arg(long)]
        verbose: bool,
    },
    /// Validate the configuration file and exit
    Validate,
}

fn main() {
    let cli = Cli::parse();
    let path = cli.config.as_path();
    let code = match cli.command {
        Commands::Serve { verbose } => serve(path, verbose),
        Commands::Validate => validate(path),
    };
    std::process::exit(code);
}

fn validate(path: &Path) -> i32 {
    match config::load(path) {
        Ok(config) => {
            println!("{}: valid configuration", path.display());
            report(&config);
            0
        }
        Err(err) => {
            eprintln!("{err}");
            2
        }
    }
}

fn serve(path: &Path, verbose: bool) -> i32 {
    let config = match config::load(path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };
    let addr = match server::bind_address(&config) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{}: configuration error: {err}", path.display());
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("internal error: {err}");
            return 3;
        }
    };
    // Signals are a process-level concern: install them here and hand the
    // shutdown future to the library, which only drains the server.
    match runtime.block_on(async {
        let shutdown = shutdown_signals()?;
        server::run(config, addr, verbose, shutdown).await
    }) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("runtime error: {err}");
            1
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
}

#[cfg(unix)]
impl ShutdownSignal {
    fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    fn forced_exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

#[cfg(unix)]
async fn recv_shutdown_signal(
    interrupt: &mut tokio::signal::unix::Signal,
    terminate: &mut tokio::signal::unix::Signal,
) -> ShutdownSignal {
    tokio::select! {
        _ = interrupt.recv() => ShutdownSignal::Interrupt,
        _ = terminate.recv() => ShutdownSignal::Terminate,
    }
}

/// Resolve on the first SIGINT/SIGTERM. A second signal abandons the drain and
/// exits with the shell's 128+signal code.
#[cfg(unix)]
fn shutdown_signals() -> std::io::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        let first = recv_shutdown_signal(&mut interrupt, &mut terminate).await;
        eprintln!("{} received; starting graceful shutdown", first.name());
        tokio::spawn(async move {
            let second = recv_shutdown_signal(&mut interrupt, &mut terminate).await;
            eprintln!("{} received again; terminating immediately", second.name());
            std::process::exit(second.forced_exit_code());
        });
    })
}

/// Resolve on the first Ctrl-C. A second Ctrl-C terminates immediately.
#[cfg(windows)]
fn shutdown_signals() -> std::io::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        let _ = ctrl_c.recv().await;
        eprintln!("Ctrl-C received; starting graceful shutdown");
        tokio::spawn(async move {
            let _ = ctrl_c.recv().await;
            eprintln!("Ctrl-C received again; terminating immediately");
            std::process::exit(130);
        });
    })
}

#[cfg(not(any(unix, windows)))]
fn shutdown_signals() -> std::io::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    Ok(std::future::pending())
}

fn report(config: &config::Config) {
    eprintln!("config_version: {}", config.config_version);
    eprintln!("server: {}:{}", config.server.bind, config.server.port);
    eprintln!("files.root: {}", config.files.root.display());
    eprintln!("routes: {}", config.routes.len());
    for route in &config.routes {
        let name = route.name.as_deref().unwrap_or("<unnamed>");
        let script = route.script.display();
        eprintln!("  {name}: {} {} -> {}", route.method, route.path, script);
    }
}
