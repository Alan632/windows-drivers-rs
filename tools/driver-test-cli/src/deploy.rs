//! Driver deployment orchestration for Windows test VMs.
//!
//! This module provides the complete workflow for deploying Windows driver packages
//! to Hyper-V virtual machines via PowerShell Direct, including:
//!
//! - **Certificate Installation**: Import test signing certificates into guest VM's Trusted People store
//! - **Driver Installation**: Deploy driver packages using `pnputil /add-driver`
//! - **Version Verification**: Parse `pnputil /enum-drivers` output to confirm installed version
//! - **Dependency Injection**: Testable architecture via [`DriverDeployer`] trait abstraction
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐
//! │  deploy_driver  │  Orchestration function (public API)
//! └────────┬────────┘
//!          │
//!          ▼
//! ┌─────────────────┐
//! │ DriverDeployer  │  Trait abstraction (dependency injection)
//! └────────┬────────┘
//!          │
//!          ├─► PnpDeployer (production: PowerShell Direct)
//!          └─► MockDeployer (testing: in-memory stub)
//! ```
//!
//! # Quick Start
//!
//! ```no_run
//! use driver_test_cli::deploy::{deploy_driver, PnpDeployer};
//! use driver_test_cli::vm::TestVm;
//! use std::path::Path;
//!
//! let deployer = PnpDeployer::default();
//! let vm = TestVm {
//!     name: "test-vm".into(),
//!     state: "Running".into(),
//!     memory_mb: 2048,
//!     cpus: 2
//! };
//!
//! let result = deploy_driver(
//!     &deployer,
//!     &vm,
//!     Some(Path::new("driver.cer")),  // Test certificate
//!     Path::new("driver.inf"),        // Driver INF
//!     Some("1.0.0.0")                 // Expected version
//! )?;
//!
//! println!("Deployed: {:?}", result.published_name);
//! # Ok::<(), driver_test_cli::deploy::DeployError>(())
//! ```
//!
//! # Dependency Injection Pattern
//!
//! For unit testing without Hyper-V infrastructure:
//!
//! ```
//! use driver_test_cli::deploy::{DriverDeployer, DriverInstallResult, DeployError, deploy_driver};
//! use driver_test_cli::vm::TestVm;
//! use std::path::Path;
//!
//! struct MockDeployer;
//!
//! impl DriverDeployer for MockDeployer {
//!     fn install_certificate(&self, _vm: &TestVm, _cert: &Path) -> Result<(), DeployError> {
//!         Ok(())
//!     }
//!
//!     fn install_driver(&self, _vm: &TestVm, inf: &Path) -> Result<DriverInstallResult, DeployError> {
//!         Ok(DriverInstallResult {
//!             inf_used: inf.display().to_string(),
//!             published_name: Some("oem123.inf".into()),
//!             version: Some("1.0.0.0".into()),
//!         })
//!     }
//!
//!     fn verify_driver_version(&self, _vm: &TestVm, _driver: &str, _expected: &str) -> Result<(), DeployError> {
//!         Ok(())
//!     }
//! }
//!
//! // Use in tests
//! let vm = TestVm { name: "test".into(), state: "Running".into(), memory_mb: 1024, cpus: 1 };
//! let result = deploy_driver(&MockDeployer, &vm, None, Path::new("test.inf"), None);
//! assert!(result.is_ok());
//! ```
//!
//! # pnputil Parser
//!
//! The [`parse_pnputil_enum_output`] function handles semi-structured text output from
//! `pnputil /enum-drivers`, supporting:
//!
//! - Multiple driver entries (segmented by blank lines or "Published Name" headers)
//! - Missing or incomplete fields (all [`DriverInfo`] fields are `Option<String>`)
//! - Basic localization (e.g., Spanish "nombre publicado")
//! - Combined "Driver Date and Version" field splitting via heuristics
//!
//! See [`parse_pnputil_enum_output`] documentation for parsing strategy, field mapping,
//! and limitations. Additional parser notes available in `docs/parser-notes.md`.
//!
//! # Error Handling
//!
//! All deployment operations return [`Result<T, DeployError>`]:
//!
//! - [`DeployError::Io`]: File not found (certificate, INF)
//! - [`DeployError::Cert`]: Certificate import failed
//! - [`DeployError::Driver`]: Driver installation failed
//! - [`DeployError::Version`]: Version mismatch
//! - [`DeployError::Ps`]: PowerShell Direct execution failure
//!
//! # Testing
//!
//! - **Unit Tests**: `tests/pnputil_parse.rs` (parser validation)
//! - **Integration Tests**: `tests/deploy_integration.rs` (mock deployer)
//! - **CLI Tests**: `tests/deploy_cli.rs` (end-to-end JSON output)
//!
//! Use `DRIVER_TEST_CLI_MOCK=1` environment variable to enable mock deployer in CLI.
//!
//! # Limitations
//!
//! - **Localization**: Parser supports basic Spanish; comprehensive localization requires extended key mappings
//! - **pnputil Format Changes**: Text parsing fragile to future Windows updates (consider WMI alternative)
//! - **PowerShell Direct**: Requires Hyper-V Integration Services enabled in guest
//! - **Certificate Handling**: Assumes certificate already accessible in guest filesystem
//!
//! See individual function documentation for detailed limitations and alternatives.

use crate::echo_test::CompanionApplication;
use crate::ps::run_ps_json;
use crate::vm::{TestVm, VmError, VmProvider};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;
use tracing::{debug, info};

/// WMI-enriched driver metadata from Win32_PnPSignedDriver
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WmiInfo {
    pub device_name: Option<String>,
    pub manufacturer: Option<String>,
    pub driver_provider_name: Option<String>,
    pub inf_name: Option<String>,
    pub is_signed: Option<bool>,
    pub signer: Option<String>,
}

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate install failed: {0}")]
    Cert(String),
    #[error("driver install failed: {0}")]
    Driver(String),
    #[error("version verification failed: expected {expected}, got {found}")]
    Version { expected: String, found: String },
    #[error("powershell error: {0}")]
    Ps(String),
    #[error("vm error: {0}")]
    Vm(#[from] VmError),
}

#[derive(Debug, Clone)]
pub struct DriverInstallResult {
    pub inf_used: String,
    pub published_name: Option<String>,
    pub version: Option<String>,
}

/// Parsed information for a single driver entry from `pnputil /enum-drivers`.
///
/// Represents a Windows third-party driver package with metadata extracted from
/// `pnputil /enum-drivers` output. All fields are optional as they depend on
/// `pnputil` output format, localization, and driver package completeness.
///
/// # Field Reliability
///
/// - **High**: `published_name`, `driver_version` (present in most entries)
/// - **Medium**: `provider`, `class`, `signer_name` (usually present for signed drivers)
/// - **Variable**: `driver_date` (format varies by locale)
///
/// # Examples
///
/// ```
/// use driver_test_cli::deploy::DriverInfo;
///
/// let info = DriverInfo {
///     published_name: Some("oem42.inf".into()),
///     provider: Some("Microsoft".into()),
///     class: Some("Display".into()),
///     driver_date: Some("11/13/2025".into()),
///     driver_version: Some("10.0.22621.1".into()),
///     signer_name: Some("Microsoft Windows Hardware Compatibility Publisher".into()),
///     raw_lines: vec!["Published Name : oem42.inf".into()],
/// };
///
/// assert_eq!(info.published_name.as_deref(), Some("oem42.inf"));
/// ```
///
/// # Localization Notes
///
/// Field presence is best-effort; localized OS builds may change key labels.
/// Parser handles basic variations (e.g., "nombre publicado" for Spanish)
/// but comprehensive localization support requires extended key mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverInfo {
    /// Published OEM driver filename (e.g., "oem123.inf").
    ///
    /// This is the primary identifier for installed third-party drivers.
    /// Windows assigns sequential OEM numbers during installation.
    pub published_name: Option<String>,

    /// Driver package provider/vendor name (e.g., "Intel Corporation").
    ///
    /// Typically matches the `Provider` field in the original INF file.
    pub provider: Option<String>,

    /// Device class (e.g., "Net", "Display", "USB").
    ///
    /// Corresponds to Windows device setup class from the INF.
    pub class: Option<String>,

    /// Driver date string (e.g., "09/26/2024").
    ///
    /// Format varies by system locale. May be combined with version
    /// in "Driver Date and Version" field or separate.
    pub driver_date: Option<String>,

    /// Driver version string (e.g., "31.0.15.4756").
    ///
    /// Version format follows driver vendor conventions.
    /// Typically matches `DriverVer` directive in INF.
    pub driver_version: Option<String>,

    /// Code signing certificate subject name.
    ///
    /// Present for signed drivers. Used to verify publisher identity.
    pub signer_name: Option<String>,

    /// Raw text lines from `pnputil` output for this driver entry.
    ///
    /// Preserved for debugging, logging, and handling unrecognized fields.
    /// Useful when parser heuristics fail or new fields are added.
    pub raw_lines: Vec<String>,
}

/// Parse `pnputil /enum-drivers` output into structured driver information entries.
///
/// This parser handles the semi-structured text output from Windows `pnputil /enum-drivers`,
/// which lists installed third-party driver packages with key-value pairs. It supports
/// multiple drivers, missing fields, and basic localization variations.
///
/// # Parsing Strategy
///
/// 1. **Group Segmentation**: Split input into driver entries using:
///    - Lines starting with "Published Name" (case-insensitive, including localized "nombre publicado")
///    - Blank line separators
/// 2. **Field Extraction**: Parse key-value pairs separated by `:` within each group
/// 3. **Key Normalization**: Convert keys to lowercase and trim whitespace for matching
/// 4. **Combined Fields**: Handle "Driver Date and Version" line by heuristic splitting:
///    - Date contains `/` (e.g., `09/26/2024`)
///    - Version contains `.` (e.g., `31.0.15.4756`)
/// 5. **Preservation**: Store raw lines in [`DriverInfo::raw_lines`] for debugging
///
/// # Arguments
///
/// * `text` - Raw text output from `pnputil /enum-drivers` command
///
/// # Returns
///
/// Vector of [`DriverInfo`] structs, one per recognized driver entry. Empty groups or
/// entries without any recognized fields are filtered out. Order matches source text.
///
/// # Field Mapping
///
/// | pnputil Output Key          | DriverInfo Field    | Notes                              |
/// |-----------------------------|---------------------|------------------------------------|
/// | Published Name              | `published_name`    | Primary identifier (e.g., oem123.inf) |
/// | Driver Package Provider     | `provider`          | Vendor name                        |
/// | Class                       | `class`             | Device class (e.g., "Net")         |
/// | Driver Date                 | `driver_date`       | Date only                          |
/// | Driver Version              | `driver_version`    | Version only                       |
/// | Driver Date and Version     | Both fields         | Combined line (split heuristically)|
/// | Signer Name                 | `signer_name`       | Code signing certificate subject  |
///
/// # Examples
///
/// ```
/// use driver_test_cli::deploy::parse_pnputil_enum_output;
///
/// let output = r#"
/// Published Name : oem123.inf
/// Driver Package Provider : Contoso, Ltd.
/// Class : Net
/// Driver Date and Version : 09/26/2024 31.0.15.4756
/// Signer Name : Contoso Code Signing CA
/// "#;
///
/// let drivers = parse_pnputil_enum_output(output);
/// assert_eq!(drivers.len(), 1);
/// assert_eq!(drivers[0].published_name.as_deref(), Some("oem123.inf"));
/// assert_eq!(drivers[0].driver_version.as_deref(), Some("31.0.15.4756"));
/// ```
///
/// # Limitations
///
/// - **Localization**: Only basic Spanish support ("nombre publicado"). Full localization
///   requires additional key mappings or pattern-based detection.
/// - **Format Changes**: Relies on `key : value` format. Future pnputil versions may break parsing.
/// - **Combined Field Splitting**: Date/version heuristic (`/` and `.` detection) may fail
///   with unusual formats (e.g., ISO dates, versions without dots).
/// - **Missing Fields**: All [`DriverInfo`] fields are `Option<String>`. Callers must handle `None`.
///
/// # Alternatives Considered
///
/// - **PowerShell Structured Output**: `Get-WindowsDriver` returns objects but unavailable in guest.
/// - **WMI Queries**: `Win32_PnPSignedDriver` provides richer data but requires additional setup.
/// - **Registry Parsing**: Direct INF database access fragile across Windows versions.
///
/// See `docs/parser-notes.md` for detailed analysis and test coverage.
pub fn parse_pnputil_enum_output(text: &str) -> Vec<DriverInfo> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let is_pub = lower.starts_with("published name") || lower.starts_with("nombre publicado"); // basic Spanish localization
        if trimmed.is_empty() {
            if !current.is_empty() {
                groups.push(current);
                current = Vec::new();
            }
            continue;
        }
        if is_pub && !current.is_empty() {
            // start new group
            groups.push(current);
            current = Vec::new();
        }
        current.push(trimmed.to_string());
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let mut out = Vec::new();
    for g in groups {
        let mut info = DriverInfo {
            published_name: None,
            provider: None,
            class: None,
            driver_date: None,
            driver_version: None,
            signer_name: None,
            raw_lines: g.clone(),
        };
        for l in &g {
            if let Some(idx) = l.find(':') {
                let (key, val_raw) = l.split_at(idx);
                let val = val_raw[1..].trim(); // skip ':'
                let key_norm = key.trim().to_ascii_lowercase();
                match key_norm.as_str() {
                    "published name" => info.published_name = Some(val.to_string()),
                    "driver package provider" | "driver package proveedor" => {
                        info.provider = Some(val.to_string())
                    }
                    "class" => info.class = Some(val.to_string()),
                    "driver version" => info.driver_version = Some(val.to_string()),
                    "driver date" => info.driver_date = Some(val.to_string()),
                    k if k.starts_with("driver date and version") => {
                        // Example: "Driver Date and Version : 09/26/2024 31.0.15.4756"
                        let parts: Vec<&str> = val.split_whitespace().collect();
                        if parts.len() >= 2 {
                            // heuristics: date contains '/', version has '.'
                            let mut date = None;
                            let mut version = None;
                            for i in 0..parts.len() {
                                let p = parts[i];
                                if date.is_none() && p.contains('/') {
                                    date = Some(p.to_string());
                                    continue;
                                }
                                if p.contains('.') {
                                    version = Some(parts[i..].join(" "));
                                    break;
                                }
                            }
                            if let Some(d) = date {
                                info.driver_date = Some(d);
                            }
                            if let Some(v) = version {
                                info.driver_version = Some(v);
                            }
                        }
                    }
                    "signer name" => info.signer_name = Some(val.to_string()),
                    _ => {}
                }
            }
        }
        // Ignore empty groups without any recognized key
        if info.published_name.is_some() || info.driver_version.is_some() || info.provider.is_some()
        {
            out.push(info);
        }
    }
    out
}

/// Abstraction for driver deployment operations supporting dependency injection.
///
/// This trait defines the contract for deploying Windows drivers to test virtual machines,
/// enabling both production implementations (PowerShell Direct to Hyper-V) and mock
/// implementations for automated testing.
///
/// # Implementations
///
/// - **[`PnpDeployer`]**: Production implementation using PowerShell Direct for real Hyper-V VMs
/// - **Mock Deployers**: Test-only implementations (see `deploy_driver` examples for patterns)
///
/// # Design Rationale
///
/// Dependency injection through this trait enables:
/// - **Fast Unit Testing**: No VM infrastructure required for tests
/// - **Hermetic Builds**: CI/CD pipelines without Hyper-V dependencies
/// - **Failure Simulation**: Test error handling paths without breaking real VMs
/// - **Cross-Platform Development**: Write tests on non-Windows hosts
///
/// # Examples
///
/// ## Production Implementation
///
/// ```no_run
/// use driver_test_cli::deploy::{DriverDeployer, DriverInstallResult, DeployError};
/// use driver_test_cli::vm::TestVm;
/// use std::path::Path;
///
/// struct PnpDeployer;
///
/// impl DriverDeployer for PnpDeployer {
///     fn install_certificate(&self, vm: &TestVm, cert: &Path) -> Result<(), DeployError> {
///         // PowerShell Direct: Import-Certificate -FilePath ... -CertStoreLocation Cert:\LocalMachine\TrustedPeople
///         unimplemented!("See PnpDeployer implementation")
///     }
///
///     fn install_driver(&self, vm: &TestVm, inf: &Path) -> Result<DriverInstallResult, DeployError> {
///         // PowerShell Direct: pnputil /add-driver <inf> /install
///         unimplemented!("See PnpDeployer implementation")
///     }
///
///     fn verify_driver_version(&self, vm: &TestVm, driver: &str, expected: &str) -> Result<(), DeployError> {
///         // PowerShell Direct: pnputil /enum-drivers | parse version
///         unimplemented!("See PnpDeployer implementation")
///     }
/// }
/// ```
///
/// ## Mock Implementation
///
/// ```
/// use driver_test_cli::deploy::{DriverDeployer, DriverInstallResult, DeployError};
/// use driver_test_cli::vm::TestVm;
/// use std::path::Path;
///
/// struct AlwaysSucceedsDeployer;
///
/// impl DriverDeployer for AlwaysSucceedsDeployer {
///     fn install_certificate(&self, _vm: &TestVm, _cert: &Path) -> Result<(), DeployError> {
///         Ok(())
///     }
///
///     fn install_driver(&self, _vm: &TestVm, inf: &Path) -> Result<DriverInstallResult, DeployError> {
///         Ok(DriverInstallResult {
///             inf_used: inf.display().to_string(),
///             published_name: Some("oem123.inf".into()),
///             version: Some("1.0.0.0".into()),
///         })
///     }
///
///     fn verify_driver_version(&self, _vm: &TestVm, _driver: &str, _expected: &str) -> Result<(), DeployError> {
///         Ok(())
///     }
/// }
///
/// // Use in tests
/// let deployer = AlwaysSucceedsDeployer;
/// let vm = TestVm { name: "test".into(), state: "Running".into(), memory_mb: 2048, cpus: 2 };
/// assert!(deployer.install_driver(&vm, Path::new("test.inf")).is_ok());
/// ```
pub trait DriverDeployer {
    /// Install a test signing certificate into the guest VM's Trusted People store.
    ///
    /// # Arguments
    ///
    /// * `vm` - Target virtual machine
    /// * `cert_path` - Path to certificate file (.cer format) accessible from host
    ///
    /// # Errors
    ///
    /// - [`DeployError::Io`] - Certificate file not found or inaccessible
    /// - [`DeployError::Cert`] - Certificate import failed (malformed cert, store access denied)
    /// - [`DeployError::Ps`] - PowerShell Direct execution failure
    fn install_certificate(&self, vm: &TestVm, cert_path: &Path) -> Result<(), DeployError>;

    /// Install a driver package into the guest VM and retrieve installation metadata.
    ///
    /// # Arguments
    ///
    /// * `vm` - Target virtual machine
    /// * `inf_path` - Path to driver INF file accessible from guest (typically copied beforehand)
    ///
    /// # Returns
    ///
    /// [`DriverInstallResult`] containing:
    /// - `published_name`: OEM INF filename (e.g., "oem42.inf")
    /// - `version`: Detected driver version (e.g., "1.2.3.4")
    /// - `inf_used`: Original INF path used for installation
    ///
    /// # Errors
    ///
    /// - [`DeployError::Io`] - INF file not found
    /// - [`DeployError::Driver`] - Installation failed (invalid INF, missing dependencies, signature issues)
    /// - [`DeployError::Ps`] - PowerShell Direct execution failure
    fn install_driver(
        &self,
        vm: &TestVm,
        inf_path: &Path,
    ) -> Result<DriverInstallResult, DeployError>;

    /// Verify that the installed driver version matches the expected version.
    ///
    /// # Arguments
    ///
    /// * `vm` - Target virtual machine
    /// * `driver_name` - Published driver name (OEM INF filename from `install_driver`)
    /// * `expected` - Expected version string (e.g., "1.0.0.0")
    ///
    /// # Errors
    ///
    /// - [`DeployError::Version`] - Installed version does not match `expected`
    /// - [`DeployError::Driver`] - Driver not found in enumeration
    /// - [`DeployError::Ps`] - PowerShell Direct execution failure
    fn verify_driver_version(
        &self,
        vm: &TestVm,
        driver_name: &str,
        expected: &str,
    ) -> Result<(), DeployError>;
}

/// Production deployer using PowerShell Direct and pnputil.
///
/// Uses OnceCell caching to avoid redundant pnputil enumeration queries.
pub struct PnpDeployer {
    /// Cached pnputil enumeration results (VM name → driver list)
    enum_cache: OnceCell<HashMap<String, String>>,
}

impl PnpDeployer {
    fn run_direct(vm: &TestVm, script: &str) -> Result<serde_json::Value, DeployError> {
        // Concatenate to avoid format! brace escaping complexity.
        let wrapped = [
            "Invoke-Command -VMName '",
            &vm.name,
            "' -ScriptBlock { ",
            script,
            " } | ConvertTo-Json -Compress",
        ]
        .concat();
        run_ps_json(&wrapped).map_err(|e| DeployError::Ps(e.to_string()))
    }
}

impl Default for PnpDeployer {
    fn default() -> Self {
        Self {
            enum_cache: OnceCell::new(),
        }
    }
}

impl DriverDeployer for PnpDeployer {
    fn install_certificate(&self, vm: &TestVm, cert_path: &Path) -> Result<(), DeployError> {
        if !cert_path.exists() {
            return Err(DeployError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cert path",
            )));
        }
        info!(vm=%vm.name, cert=%cert_path.display(), "installing test certificate");
        // Copy file then import. Simplified: assume cert already present in guest path.
        let guest_path = cert_path.display().to_string();
        // Simplify: output thumbprint or ERROR string; JSON conversion wrapper will convert scalar.
        let script = [
			"try { (Import-Certificate -FilePath '", &guest_path,
			"' -CertStoreLocation Cert:\\LocalMachine\\TrustedPeople).Thumbprint } catch { 'ERROR' }"
		].concat();
        let val = Self::run_direct(vm, &script)?;
        if val.is_string() && val.as_str() != Some("ERROR") {
            Ok(())
        } else {
            Err(DeployError::Cert(val.to_string()))
        }
    }

    fn install_driver(
        &self,
        vm: &TestVm,
        inf_path: &Path,
    ) -> Result<DriverInstallResult, DeployError> {
        if !inf_path.exists() {
            return Err(DeployError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "inf path",
            )));
        }
        info!(vm=%vm.name, inf=%inf_path.display(), "installing driver via pnputil");
        debug!("step 1/3: executing pnputil /add-driver");
        let guest_inf = inf_path.display().to_string();
        let script = [
            "try { pnputil /add-driver '",
            &guest_inf,
            "' /install | Out-String } catch { $_.Exception.Message }",
        ]
        .concat();
        let val = Self::run_direct(vm, &script)?;
        let raw = val.to_string();
        if raw.to_ascii_lowercase().contains("add-driver")
            || raw.to_ascii_lowercase().contains("published")
        {
            debug!("step 2/3: querying installed drivers for version detection");
            // Query enumerated drivers
            let enum_script = "pnputil /enum-drivers | Out-String";
            let enum_val = Self::run_direct(vm, enum_script)?;
            let enum_str = enum_val.to_string();
            // Heuristic parse for Published Name and Driver Version lines
            let infos = parse_pnputil_enum_output(&enum_str);
            debug!(vm=%vm.name, entries=%infos.len(), "parsed pnputil enumeration entries");
            debug!("step 3/3: matching driver entry to INF filename");
            // Attempt to select the most recent (last) entry whose raw block mentions the INF file name
            let inf_file = inf_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mut selected: Option<DriverInfo> = None;
            for info in infos.iter().rev() {
                // iterate from end as newest installs tend to appear later
                let mut hit = false;
                for rl in &info.raw_lines {
                    if rl.to_ascii_lowercase().contains(&inf_file) {
                        hit = true;
                        break;
                    }
                }
                if hit {
                    selected = Some(info.clone());
                    break;
                }
            }
            let published = selected
                .as_ref()
                .and_then(|i| i.published_name.clone())
                .or_else(|| {
                    // fallback: parse installation output directly
                    raw.lines().find_map(|l| {
                        let lt = l.trim();
                        if lt.to_ascii_lowercase().starts_with("published name") {
                            lt.split(':').nth(1).map(|s| s.trim().to_string())
                        } else {
                            None
                        }
                    })
                });
            let version = selected.as_ref().and_then(|i| i.driver_version.clone());
            Ok(DriverInstallResult {
                inf_used: inf_path.display().to_string(),
                published_name: published,
                version,
            })
        } else {
            Err(DeployError::Driver(raw))
        }
    }

    fn verify_driver_version(
        &self,
        vm: &TestVm,
        driver_name: &str,
        expected: &str,
    ) -> Result<(), DeployError> {
        info!(vm=%vm.name, driver=%driver_name, expected=%expected, "verifying driver version");

        // Try cache first to avoid redundant enumeration
        let enum_script = "pnputil /enum-drivers | Out-String";
        let cache_key = vm.name.clone();

        let enum_str = self
            .enum_cache
            .get_or_try_init(|| -> Result<HashMap<String, String>, DeployError> {
                debug!(vm=%vm.name, "cache miss, enumerating drivers");
                let enum_val = Self::run_direct(vm, enum_script)?;
                let output = enum_val.to_string();
                let mut map = HashMap::new();
                map.insert(cache_key.clone(), output.clone());
                Ok(map)
            })?
            .get(&cache_key)
            .cloned()
            .unwrap_or_default();

        // Parse full driver enumeration and find matching driver
        let infos = parse_pnputil_enum_output(&enum_str);
        let matching = infos
            .iter()
            .find(|info| info.published_name.as_deref() == Some(driver_name));

        match matching {
            Some(info) => {
                let found_version = info.driver_version.as_deref().unwrap_or("<unknown>");
                if found_version == expected {
                    debug!(driver=%driver_name, version=%found_version, "version verification successful");
                    Ok(())
                } else {
                    Err(DeployError::Version {
                        expected: expected.to_string(),
                        found: found_version.to_string(),
                    })
                }
            }
            None => Err(DeployError::Driver(format!(
                "Driver {} not found in enumeration",
                driver_name
            ))),
        }
    }
}

/// Deploy a driver package to a test VM with optional certificate and version verification.
///
/// This orchestration function coordinates the complete driver deployment workflow:
/// certificate installation (if provided), driver installation via `pnputil`, and
/// optional version verification. It abstracts the deployment provider to support
/// both real Hyper-V operations and mock testing scenarios.
///
/// # Arguments
///
/// * `prov` - Implementation of [`DriverDeployer`] trait (e.g., [`PnpDeployer`] for real deployments)
/// * `vm` - Target virtual machine for deployment
/// * `cert` - Optional path to test signing certificate (.cer file) for driver signing verification
/// * `inf` - Path to driver INF file (must exist and be accessible from guest VM)
/// * `expected_version` - Optional version string for verification (e.g., "1.2.3.4")
///
/// # Returns
///
/// [`DriverInstallResult`] containing the published OEM filename and detected version,
/// or [`DeployError`] if any step fails.
///
/// # Errors
///
/// - [`DeployError::Io`] - Certificate or INF file not found
/// - [`DeployError::Cert`] - Certificate installation failed in guest
/// - [`DeployError::Driver`] - Driver installation failed (e.g., invalid INF, missing dependencies)
/// - [`DeployError::Version`] - Installed version doesn't match `expected_version`
/// - [`DeployError::Ps`] - PowerShell Direct execution failure
///
/// # Examples
///
/// ```no_run
/// use driver_test_cli::deploy::{deploy_driver, PnpDeployer};
/// use driver_test_cli::vm::TestVm;
/// use std::path::Path;
///
/// let deployer = PnpDeployer::default();
/// let vm = TestVm { name: "test-vm".into(), state: "Running".into(), memory_mb: 2048, cpus: 2 };
/// let cert_path = Path::new("driver.cer");
/// let inf_path = Path::new("driver.inf");
///
/// let result = deploy_driver(
///     &deployer,
///     &vm,
///     Some(cert_path),
///     inf_path,
///     Some("1.0.0.0")
/// );
///
/// match result {
///     Ok(install) => println!("Deployed as {}", install.published_name.unwrap_or_default()),
///     Err(e) => eprintln!("Deployment failed: {}", e),
/// }
/// ```
///
/// # Workflow
///
/// 1. **Certificate Installation** (if `cert` provided):
///    - Copy certificate to guest VM
///    - Import into `Cert:\LocalMachine\TrustedPeople` store
/// 2. **Driver Installation**:
///    - Execute `pnputil /add-driver <inf> /install` in guest
///    - Parse installation output for published OEM filename
///    - Query `pnputil /enum-drivers` to extract version
/// 3. **Version Verification** (if `expected_version` provided):
///    - Compare detected version against expected
///    - Return error if mismatch
///
/// # Testing
///
/// Use dependency injection with mock deployer for unit testing:
///
/// ```
/// use driver_test_cli::deploy::{deploy_driver, DriverDeployer, DriverInstallResult, DeployError};
/// use driver_test_cli::vm::TestVm;
/// use std::path::Path;
///
/// struct MockDeployer;
/// impl DriverDeployer for MockDeployer {
///     fn install_certificate(&self, _vm: &TestVm, _cert: &Path) -> Result<(), DeployError> {
///         Ok(())
///     }
///     fn install_driver(&self, _vm: &TestVm, _inf: &Path) -> Result<DriverInstallResult, DeployError> {
///         Ok(DriverInstallResult {
///             inf_used: "test.inf".into(),
///             published_name: Some("oem999.inf".into()),
///             version: Some("1.0.0.0".into()),
///         })
///     }
///     fn verify_driver_version(&self, _vm: &TestVm, _driver: &str, _expected: &str) -> Result<(), DeployError> {
///         Ok(())
///     }
/// }
///
/// let vm = TestVm { name: "mock".into(), state: "Running".into(), memory_mb: 1024, cpus: 1 };
/// let result = deploy_driver(&MockDeployer, &vm, None, Path::new("mock.inf"), Some("1.0.0.0"));
/// assert!(result.is_ok());
/// ```
/// Query WMI metadata for an installed driver using Win32_PnPSignedDriver.
///
/// # Arguments
///
/// * `vm` - Target virtual machine
/// * `published_name` - OEM driver filename (e.g., "oem123.inf")
///
/// # Returns
///
/// [`WmiInfo`] with enriched metadata if found, or [`DeployError`] if query fails.
pub fn query_wmi_info(vm: &TestVm, published_name: &str) -> Result<WmiInfo, DeployError> {
    debug!(vm=%vm.name, driver=%published_name, "querying WMI metadata");

    let script = format!(
		"Get-WmiObject Win32_PnPSignedDriver | Where-Object {{ $_.InfName -eq '{published_name}' }} | Select-Object DeviceName,Manufacturer,DriverProviderName,InfName,IsSigned,Signer | ConvertTo-Json",
		published_name = published_name
	);

    let wrapped = [
        "Invoke-Command -VMName '",
        &vm.name,
        "' -ScriptBlock { ",
        &script,
        " }",
    ]
    .concat();
    let val = run_ps_json(&wrapped).map_err(|e| DeployError::Ps(e.to_string()))?;

    let device_name = val
        .get("DeviceName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let manufacturer = val
        .get("Manufacturer")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let driver_provider_name = val
        .get("DriverProviderName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let inf_name = val
        .get("InfName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let is_signed = val.get("IsSigned").and_then(|v| v.as_bool());
    let signer = val
        .get("Signer")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(WmiInfo {
        device_name,
        manufacturer,
        driver_provider_name,
        inf_name,
        is_signed,
        signer,
    })
}

pub fn copy_application<P: VmProvider>(
    prov: &P,
    vm: &TestVm,
    app: &CompanionApplication,
) -> Result<String, DeployError> {
    let remote = app.remote_path();
    prov.copy_file(vm, &app.executable_path, &remote)
        .map_err(DeployError::from)?;
    Ok(remote)
}

pub fn deploy_driver<P: DriverDeployer>(
    prov: &P,
    vm: &TestVm,
    cert: Option<&Path>,
    inf: &Path,
    expected_version: Option<&str>,
) -> Result<DriverInstallResult, DeployError> {
    if let Some(c) = cert {
        prov.install_certificate(vm, c)?;
    }
    let res = prov.install_driver(vm, inf)?;
    if let Some(exp) = expected_version {
        if let Some(found) = res.version.as_deref() {
            if exp != found {
                return Err(DeployError::Version {
                    expected: exp.to_string(),
                    found: found.to_string(),
                });
            }
        }
        if let Some(published) = res.published_name.as_deref() {
            prov.verify_driver_version(vm, published, exp)?;
        }
    }
    Ok(res)
}
