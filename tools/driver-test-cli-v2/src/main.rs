// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use driver_test_cli_v2::error::{EXIT_INFRA_ERROR, EXIT_SUCCESS, EXIT_TEST_FAILURE};
use driver_test_cli_v2::pipeline::{Pipeline, PipelineConfig};
use driver_test_cli_v2::ps_runner::VmCredential;

/// Default VM name when none is supplied via `--vm-name`.
const DEFAULT_VM_NAME: &str = "driver-test-vm";

#[derive(Parser)]
#[command(
    name = "driver-smoketest",
    version,
    about = "Smoketest a Windows driver on a Hyper-V VM"
)]
struct Cli {
    /// Path to the driver package directory (must contain .inf and .sys).
    #[arg(long, required = true)]
    driver_package: PathBuf,

    /// Hyper-V VM name to target.
    #[arg(long)]
    vm_name: Option<String>,

    /// Skip the driver verification step.
    #[arg(long)]
    skip_verify: bool,

    /// Skip the trace capture step.
    #[arg(long)]
    skip_capture: bool,

    /// Duration in seconds for ETW trace capture.
    #[arg(long, default_value_t = 30)]
    capture_duration: u32,

    /// Do not clean up temporary resources on completion.
    #[arg(long)]
    no_cleanup: bool,

    /// Host directory where captured log files will be placed.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Print the report as JSON instead of interactive output.
    #[arg(long)]
    json: bool,

    /// Increase verbosity (0=warn, 1=info, 2=debug, 3+=trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialize tracing subscriber based on verbosity level.
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .init();

    // Hardcoded test VM credentials
    let credential = Some(VmCredential {
        username: "DriverTestAdmin".to_string(),
        password: "1234".to_string(),
    });

    let vm_name = cli.vm_name.unwrap_or_else(|| DEFAULT_VM_NAME.to_string());

    let config = PipelineConfig {
        driver_package: cli.driver_package,
        vm_name,
        credential,
        skip_verify: cli.skip_verify,
        skip_capture: cli.skip_capture,
        capture_duration: cli.capture_duration,
        no_cleanup: cli.no_cleanup,
        output_dir: cli
            .output_dir
            .unwrap_or_else(|| std::env::temp_dir().join("driver-test-logs")),
    };

    // Create the pipeline (validates the driver package on construction).
    let mut pipeline = match Pipeline::new(config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{}", e.action());
            return ExitCode::from(e.exit_code() as u8);
        }
    };

    // Run all pipeline steps.
    let report = pipeline.run();

    // Output the report.
    if cli.json {
        println!("{}", report.to_json());
    } else {
        report.print_interactive();
    }

    // Exit with appropriate code.
    if report.is_success() {
        ExitCode::from(EXIT_SUCCESS as u8)
    } else if report.has_infra_failure() {
        ExitCode::from(EXIT_INFRA_ERROR as u8)
    } else {
        ExitCode::from(EXIT_TEST_FAILURE as u8)
    }
}
