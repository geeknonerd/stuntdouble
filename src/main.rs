//! Command line surface for stuntdouble.
//! Contracts: docs/contracts/cli.md (commands, flags, exit codes)
//!   0 success, 1 runtime error, 2 configuration error, 3 internal error

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

mod config;
mod matcher;
mod server;

const DEFAULT_CONFIG: &str = "stuntdouble.toml";

#[derive(Parser)]
#[command(
    name = "stuntdouble",
    version,
    about = "Mock server for integration testing"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the mock server
    Serve {
        /// Configuration file
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Attach a detail field to 502/500 responses
        #[arg(long)]
        verbose: bool,
    },
    /// Validate the configuration file and exit
    Validate {
        /// Configuration file
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Serve { config, verbose } => serve(&config, verbose),
        Commands::Validate { config } => validate(&config),
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
    // tradeoff: --verbose is accepted now so runbooks can pin the flag; the
    // detail payload itself lands with the observability slice (T7).
    let _ = verbose;
    let addr = match server::bind_address(&config) {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("{}: configuration error: {err}", path.display());
            return 2;
        }
    };
    eprintln!("stuntdouble listening on http://{addr}");
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
    match runtime.block_on(server::run(config)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("runtime error: {err}");
            1
        }
    }
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
