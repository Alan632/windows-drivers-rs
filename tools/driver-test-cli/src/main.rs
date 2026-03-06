mod cli;
mod config;
mod debug;
mod deploy;
mod driver_detect;
mod echo_test;
mod errors;
mod output;
mod package;
mod ps;
mod vm;

use clap::Parser;
use cli::Cli;
use errors::AppError;
use std::path::PathBuf;
use std::process;
use tracing::Level;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn init_tracing(verbosity: u8, json: bool) {
    let level = match verbosity {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };
    let filter = EnvFilter::new(level.to_string());
    if json {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().flatten_event(true))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .init();
    }
}

fn main() {
    let cli = Cli::parse();
    let config_path = std::env::var("DRIVER_TEST_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(config::DEFAULT_CONFIG_FILE));
    let (config, config_err) = match config::RootConfig::maybe_load(&config_path) {
        Ok(cfg) => (cfg, None),
        Err(err) => (None, Some(err)),
    };

    let config_vm_name = config
        .as_ref()
        .and_then(|cfg| cfg.vm.as_ref())
        .and_then(|vm| vm.name.clone());
    let vm_name = cli.vm_name.clone().or(config_vm_name);

    let verbosity_from_config = config
        .as_ref()
        .and_then(|cfg| cfg.defaults.as_ref())
        .and_then(|d| d.verbosity.as_deref())
        .and_then(parse_verbosity_hint)
        .unwrap_or(0);
    let verbosity = if cli.verbose == 0 {
        verbosity_from_config
    } else {
        cli.verbose
    };
    init_tracing(verbosity, cli.json);

    if let Some(err) = config_err {
        tracing::warn!(path=%config_path.display(), %err, "failed to load config file");
    }
    if let Some(cfg) = config.as_ref() {
        if let Some(vm_cfg) = cfg.vm.as_ref() {
            tracing::debug!(vm = ?vm_cfg.name, memory_mb = ?vm_cfg.memory_mb, cpus = ?vm_cfg.cpus, snapshot = ?vm_cfg.baseline_snapshot, "config VM defaults loaded");
        }
        if let Some(defaults) = cfg.defaults.as_ref() {
            tracing::debug!(verbosity = ?defaults.verbosity, retry_flaky = ?defaults.retry_flaky, timeout_secs = ?defaults.timeout_secs, "config general defaults loaded");
        }
    }

    let vm_name_ref = vm_name.as_deref();
    tracing::info!(command=?cli.command, vm_name=?vm_name_ref, "starting driver-test-cli");
    let result = match cli.command {
        cli::Commands::Test(cmd) => cmd.run(vm_name_ref),
        cli::Commands::Setup(cmd) => cmd.run(),
        cli::Commands::Snapshot(cmd) => cmd.run(vm_name_ref),
        cli::Commands::Clean(cmd) => cmd.run(vm_name_ref),
        cli::Commands::Deploy(cmd) => cmd.run(vm_name_ref, cli.json),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        let exit_code = match e.downcast_ref::<AppError>() {
            Some(AppError::Detection(_)) => 1, // User error
            Some(AppError::Vm(_)) | Some(AppError::Deploy(_)) => 2, // System error
            _ => 1,                            // Default to user error
        };
        process::exit(exit_code);
    }
}

fn parse_verbosity_hint(value: &str) -> Option<u8> {
    match value.to_ascii_lowercase().as_str() {
        "warn" | "warning" => Some(0),
        "info" => Some(1),
        "debug" => Some(2),
        "trace" => Some(3),
        _ => value.parse::<u8>().ok(),
    }
}
