//! Library surface for internal modules so tests can exercise detection logic.
pub mod cli; // expose CLI types (DeployCommand, Cli) for integration-style tests
pub mod debug;
pub mod deploy;
pub mod driver_detect;
pub mod echo_test;
pub mod errors;
pub mod output;
pub mod package;
pub mod ps;
pub mod vm; // expose structured output types (DeployResult)
