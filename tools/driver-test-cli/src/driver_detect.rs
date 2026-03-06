use crate::echo_test::CompanionApplication;
use crate::package::{DriverPackage, DriverType, RepositoryType};
use regex::Regex;
use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;
use toml::Value;
use walkdir::WalkDir;

fn read_utf8(path: &Path) -> Result<String, DetectionError> {
    let data = fs::read(path)?;
    String::from_utf8(data).map_err(|_| DetectionError::Utf8)
}

#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid utf8")]
    Utf8,
    #[error("not found")]
    NotFound,
}

pub fn detect_driver_type(
    root: &Path,
    override_type: Option<&str>,
) -> Result<DriverPackage, DetectionError> {
    let repository = RepositoryType::detect(root);
    // 1. Explicit override wins.
    if let Some(o) = override_type {
        return Ok(DriverPackage::new(
            root.to_path_buf(),
            repository,
            parse_override(o),
            None,
            None,
        ));
    }

    // 2. Cargo metadata (authoritative if present)
    let cargo = root.join("Cargo.toml");
    if cargo.exists() {
        let content = read_utf8(&cargo)?;
        if let Some(dt) = scan_cargo_metadata(&content) {
            return Ok(DriverPackage::new(
                root.to_path_buf(),
                repository,
                dt,
                None,
                None,
            ));
        }
        // 3. Kernel-like heuristic fallback (panic=abort + no_std) → WDM
        if is_kernel_like(&content) {
            return Ok(DriverPackage::new(
                root.to_path_buf(),
                repository,
                DriverType::Wdm,
                None,
                None,
            ));
        }
    }

    // 4. INF / INX scanning with version extraction.
    if let Some((dt, ver, inf_path)) = scan_inf(root, repository)? {
        return Ok(DriverPackage::new(
            root.to_path_buf(),
            repository,
            dt,
            ver,
            Some(inf_path),
        ));
    }

    Err(DetectionError::NotFound)
}

/// Return true when the current working directory belongs to the
/// Windows-Rust-driver-samples repository hierarchy.
pub fn detect_samples_repository(root: &Path) -> bool {
    matches!(
        RepositoryType::detect(root),
        RepositoryType::WindowsRustDriverSamples
    )
}

fn parse_override(v: &str) -> DriverType {
    match v.to_ascii_uppercase().as_str() {
        "KMDF" => DriverType::Kmdf,
        "UMDF" => DriverType::Umdf,
        "WDM" => DriverType::Wdm,
        _ => DriverType::Wdm,
    }
}

fn scan_cargo_metadata(content: &str) -> Option<DriverType> {
    // Accept metadata in either [package.metadata.wdk] or direct [package.metadata]
    // Example: driver-type = "KMDF"
    let re = Regex::new(r#"(?i)driver-type\s*=\s*"(KMDF|UMDF|WDM)""#).ok()?; // (?i) case-insensitive
    let caps = re.captures(content)?;
    let val = &caps[1];
    match val.to_ascii_uppercase().as_str() {
        "KMDF" => Some(DriverType::Kmdf),
        "UMDF" => Some(DriverType::Umdf),
        "WDM" => Some(DriverType::Wdm),
        _ => None,
    }
}

fn is_kernel_like(content: &str) -> bool {
    content.contains("panic = \"abort\"") && content.contains("no_std")
}

fn scan_inf(
    root: &Path,
    repo: RepositoryType,
) -> Result<Option<(DriverType, Option<String>, PathBuf)>, DetectionError> {
    let depth = if matches!(repo, RepositoryType::WindowsRustDriverSamples) {
        8
    } else {
        5
    };
    for search_root in candidate_inf_roots(root, repo) {
        if !search_root.exists() {
            continue;
        }
        for entry in WalkDir::new(search_root)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let ext = match path.extension().and_then(|s| s.to_str()) {
                Some(e) => e.to_ascii_lowercase(),
                None => continue,
            };
            if ext == "inf" || ext == "inx" {
                let data = match read_utf8(path) {
                    Ok(d) => d,
                    Err(DetectionError::Utf8) => return Err(DetectionError::Utf8),
                    Err(_) => continue,
                };
                let lowered = data.to_ascii_lowercase();
                if let Some(dt) = infer_driver_type_from_inf(&lowered) {
                    let version = extract_inf_driverver(&lowered);
                    return Ok(Some((dt, version, path.to_path_buf())));
                }
            }
        }
    }
    Ok(None)
}

fn extract_inf_driverver(inf_lower: &str) -> Option<String> {
    // DriverVer=MM/DD/YYYY,major.minor.build.revision
    for line in inf_lower.lines() {
        let line = line.trim();
        if line.starts_with("driverver=") {
            // split at comma; take part after comma
            if let Some((_, rest)) = line.split_once(',') {
                let ver = rest.trim();
                // basic sanity: contains at least one '.' digit pattern
                if ver.chars().any(|c| c == '.') {
                    return Some(ver.to_string());
                } else {
                    return Some(ver.to_string()); // still return raw if unusual
                }
            }
        }
    }
    None
}

fn candidate_inf_roots(root: &Path, repo: RepositoryType) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    if matches!(repo, RepositoryType::WindowsRustDriverSamples) {
        if let Some(parent) = root.parent() {
            push_unique(&mut roots, parent.to_path_buf());
        }
        let mut hints = vec![
            "driver",
            "drivers",
            "inf",
            "pkg",
            "package",
            "deployment",
            "deployment/package",
        ];
        for hint in hints.drain(..) {
            let path = if hint.contains('/') {
                let mut acc = root.to_path_buf();
                for part in hint.split('/') {
                    acc = acc.join(part);
                }
                acc
            } else {
                root.join(hint)
            };
            push_unique(&mut roots, path);
            if let Some(parent) = root.parent() {
                push_unique(&mut roots, parent.join(hint));
            }
        }
    }
    roots
}

fn push_unique(vec: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !vec.iter().any(|p| p == &candidate) {
        vec.push(candidate);
    }
}

fn infer_driver_type_from_inf(lowered: &str) -> Option<DriverType> {
    if lowered.contains("[kmdf]") || lowered.contains("kmdflibraryversion") {
        Some(DriverType::Kmdf)
    } else if lowered.contains("[umdf]") || lowered.contains("umdflibraryversion") {
        Some(DriverType::Umdf)
    } else if lowered.contains("[version]") {
        Some(DriverType::Wdm)
    } else {
        None
    }
}

pub fn locate_companion_application(
    package: &DriverPackage,
) -> Result<Option<CompanionApplication>, DetectionError> {
    let cargo_bins = parse_cargo_bins(&package.root).unwrap_or_default();
    let mut candidates: Vec<PathBuf> = Vec::new();
    let build_dir = package.build_output_dir();
    for bin in cargo_bins {
        let candidate = build_dir.join(format!("{bin}.exe"));
        candidates.push(candidate);
    }
    let mut search_dirs = vec!["bin", "exe", "apps", "application", "companion"];
    if matches!(package.repository, RepositoryType::WindowsRustDriverSamples) {
        search_dirs.extend([
            "driver/bin",
            "driver/apps",
            "app",
            "apps/win32",
            "host",
            "samples",
        ]);
    }
    for dir in search_dirs {
        let mut base = package.root.clone();
        for segment in dir.split('/') {
            if segment.is_empty() {
                continue;
            }
            base = base.join(segment);
        }
        if base.exists() {
            for entry in WalkDir::new(&base)
                .max_depth(3)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("exe"))
                    .unwrap_or(false)
                {
                    candidates.push(path.to_path_buf());
                }
            }
        }
    }
    for candidate in candidates {
        if candidate.exists() {
            let patterns = load_pattern_file(&package.root).unwrap_or_else(default_echo_patterns);
            return Ok(Some(CompanionApplication::new(candidate, patterns)));
        }
    }
    Ok(None)
}

fn parse_cargo_bins(root: &Path) -> Result<Vec<String>, DetectionError> {
    let manifest = root.join("Cargo.toml");
    if !manifest.exists() {
        return Ok(Vec::new());
    }
    let content = read_utf8(&manifest)?;
    let value: Value = toml::from_str(&content).map_err(|_| DetectionError::NotFound)?;
    let mut bins = Vec::new();
    if let Some(bin_entries) = value.get("bin").and_then(|v| v.as_array()) {
        for entry in bin_entries {
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                bins.push(name.to_string());
            }
        }
    }
    if bins.is_empty() {
        if let Some(package) = value.get("package") {
            if let Some(name) = package.get("name").and_then(|n| n.as_str()) {
                bins.push(name.replace('-', "_"));
            }
        }
    }
    Ok(bins)
}

fn load_pattern_file(root: &Path) -> Option<Vec<String>> {
    let pattern_files = [
        "echo_patterns.txt",
        "companion_patterns.txt",
        "tests/echo_patterns.txt",
    ];
    for rel in &pattern_files {
        let path = root.join(rel);
        if path.exists() {
            let data = read_utf8(&path).ok()?;
            let patterns: Vec<String> = data
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|s| s.to_string())
                .collect();
            if !patterns.is_empty() {
                return Some(patterns);
            }
        }
    }
    None
}

fn default_echo_patterns() -> Vec<String> {
    vec![
        "echo: sending packet".into(),
        "echo: received packet".into(),
    ]
}
