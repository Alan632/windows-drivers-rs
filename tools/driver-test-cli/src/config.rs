use serde::Deserialize;
use std::{fs, path::Path};
use thiserror::Error;

pub const DEFAULT_CONFIG_FILE: &str = "driver-test.toml";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Deserialize, Default)]
pub struct VmConfig {
    pub name: Option<String>,
    pub cpus: Option<u8>,
    pub memory_mb: Option<u32>,
    pub baseline_snapshot: Option<String>,
}
#[derive(Debug, Deserialize, Default)]
pub struct Defaults {
    pub verbosity: Option<String>,
    pub retry_flaky: Option<bool>,
    pub timeout_secs: Option<u64>,
}
#[derive(Debug, Deserialize, Default)]
pub struct RootConfig {
    pub vm: Option<VmConfig>,
    pub defaults: Option<Defaults>,
}

impl RootConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let data = fs::read_to_string(path)?;
        Ok(toml::from_str(&data)?)
    }

    pub fn maybe_load(path: &Path) -> Result<Option<Self>, ConfigError> {
        if path.exists() {
            Self::load(path).map(Some)
        } else {
            Ok(None)
        }
    }
}
