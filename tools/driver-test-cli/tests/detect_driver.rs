use assert_fs::prelude::*;
use assert_fs::TempDir;
use driver_test_cli::{driver_detect::detect_driver_type, package::DriverType};
use std::fs;
use std::io::Write; // for write_all

#[test]
fn override_wins() {
    let tmp = TempDir::new().unwrap();
    let pkg = detect_driver_type(tmp.path(), Some("KMDF")).unwrap();
    assert_eq!(pkg.driver_type, DriverType::Kmdf);
    assert!(pkg.version.is_none());
}

#[test]
fn cargo_metadata_detects_umdf() {
    let tmp = TempDir::new().unwrap();
    let cargo = tmp.child("Cargo.toml");
    cargo.write_str(r#"[package]\nname="x"\nversion="0.0.1"\n[package.metadata.wdk]\ndriver-type = "UMDF"\n"#).unwrap();
    let pkg = detect_driver_type(tmp.path(), None).unwrap();
    assert_eq!(pkg.driver_type, DriverType::Umdf);
    assert!(pkg.inf_path.is_none());
}

#[test]
fn kernel_like_fallback_wdm() {
    let tmp = TempDir::new().unwrap();
    let cargo = tmp.child("Cargo.toml");
    cargo.write_str(r#"[package]\nname="x"\nversion="0.0.1"\n[lib]\ncrate-type=["cdylib"]\n[profile.release]\npanic = "abort"\n[dependencies]\n"#).unwrap();
    // add no_std indicator somewhere (simulate)
    let lib_rs = tmp.child("src/lib.rs");
    lib_rs.write_str("#![no_std]\n").unwrap();
    // detection relies only on Cargo.toml content for kernel_like; ensure 'no_std' present
    // Append no_std to cargo file
    fs::OpenOptions::new()
        .append(true)
        .open(cargo.path())
        .unwrap()
        .write_all(b"\n# no_std\n")
        .unwrap();
    let pkg = detect_driver_type(tmp.path(), None).unwrap();
    assert_eq!(pkg.driver_type, DriverType::Wdm);
}

#[test]
fn inf_detects_kmdf_case_insensitive_and_version() {
    let tmp = TempDir::new().unwrap();
    // No cargo metadata -> rely on INF
    let inf = tmp.child("sample.inf");
    inf.write_str("[Kmdf]\nDriverVer=06/01/2024,1.2.3.4\n")
        .unwrap();
    let pkg = detect_driver_type(tmp.path(), None).unwrap();
    assert_eq!(pkg.driver_type, DriverType::Kmdf);
    assert_eq!(pkg.version.as_deref(), Some("1.2.3.4"));
    assert!(pkg.inf_path.as_ref().unwrap().ends_with("sample.inf"));
}

#[test]
fn inf_detects_umdf_lowercase_section() {
    let tmp = TempDir::new().unwrap();
    let inf = tmp.child("drv.inx");
    inf.write_str("[umdf]\nDriverVer=01/01/2025,10.20.30.40\n")
        .unwrap();
    let pkg = detect_driver_type(tmp.path(), None).unwrap();
    assert_eq!(pkg.driver_type, DriverType::Umdf);
    assert_eq!(pkg.version.as_deref(), Some("10.20.30.40"));
}

#[test]
fn not_found_error() {
    let tmp = TempDir::new().unwrap();
    let err = detect_driver_type(tmp.path(), None).unwrap_err();
    match err {
        driver_test_cli::driver_detect::DetectionError::NotFound => {}
        _ => panic!("unexpected error variant"),
    }
}
