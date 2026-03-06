use std::path::{Path, PathBuf};

/// Supported source repository layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryType {
    WindowsDriversRs,
    WindowsRustDriverSamples,
}

impl RepositoryType {
    /// Detect repository type by walking ancestor directories for known markers.
    ///
    /// Heuristics prefer explicit folder names (e.g., `windows-rust-driver-samples`) but
    /// fall back to marker files used by samples repo metadata such as `samples.json` or
    /// `.samples-root`.
    pub fn detect(root: &Path) -> Self {
        let mut current = Some(root);
        while let Some(dir) = current {
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                if name.eq_ignore_ascii_case("windows-rust-driver-samples") {
                    return RepositoryType::WindowsRustDriverSamples;
                }
            }
            if RepositoryType::has_samples_marker(dir) {
                return RepositoryType::WindowsRustDriverSamples;
            }
            current = dir.parent();
        }
        RepositoryType::WindowsDriversRs
    }

    fn has_samples_marker(dir: &Path) -> bool {
        const MARKERS: [&str; 5] = [
            "samples.json",
            "sample-list.json",
            "Samples.props",
            ".samples-root",
            "samples.yaml",
        ];
        MARKERS.iter().any(|marker| dir.join(marker).exists())
    }

    /// Return preferred build output directory for the repository type.
    ///
    /// Windows-Rust-driver-samples defaults to Cargo WDK layout (`target/wdk/<arch>/Release`).
    /// The helper returns the first existing candidate, falling back to the conventional path
    /// even if it has not been created yet so callers can join filenames deterministically.
    pub fn build_output_dir(&self, root: &Path) -> PathBuf {
        let mut candidates: Vec<PathBuf> = match self {
            RepositoryType::WindowsDriversRs => vec![root.join("target").join("release")],
            RepositoryType::WindowsRustDriverSamples => vec![
                root.join("target").join("wdk").join("x64").join("Release"),
                root.join("target")
                    .join("wdk")
                    .join("amd64")
                    .join("Release"),
                root.join("target").join("release"),
            ],
        };
        if let RepositoryType::WindowsRustDriverSamples = self {
            candidates.push(root.join("artifacts"));
        }
        for cand in &candidates {
            if cand.exists() {
                return cand.clone();
            }
        }
        candidates.pop().unwrap_or_else(|| root.join("target"))
    }
}

/// Driver frameworks supported by the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverType {
    Kmdf,
    Umdf,
    Wdm,
}

/// Detected driver package metadata shared across modules.
#[derive(Debug, Clone)]
pub struct DriverPackage {
    pub root: PathBuf,
    pub repository: RepositoryType,
    pub driver_type: DriverType,
    pub version: Option<String>,
    pub inf_path: Option<PathBuf>,
}

impl DriverPackage {
    pub fn new(
        root: PathBuf,
        repository: RepositoryType,
        driver_type: DriverType,
        version: Option<String>,
        inf_path: Option<PathBuf>,
    ) -> Self {
        Self {
            root,
            repository,
            driver_type,
            version,
            inf_path,
        }
    }

    pub fn build_output_dir(&self) -> PathBuf {
        self.repository.build_output_dir(&self.root)
    }
}
