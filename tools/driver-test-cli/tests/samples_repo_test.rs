use assert_fs::{prelude::*, TempDir};
use driver_test_cli::driver_detect::{detect_driver_type, detect_samples_repository};
use driver_test_cli::package::{DriverPackage, DriverType, RepositoryType};

#[test]
fn samples_repo_detection_identifies_layout() {
    let tmp = TempDir::new().unwrap();
    let driver_dir = tmp.child("windows-rust-driver-samples/general/echo/kmdf/driver");
    driver_dir.create_dir_all().unwrap();
    driver_dir
        .child("Cargo.toml")
        .write_str("[package]\nname=\"echo\"\nversion=\"0.1.0\"\n")
        .unwrap();

    let inf_dir = tmp.child("windows-rust-driver-samples/general/echo/kmdf/inf");
    inf_dir.create_dir_all().unwrap();
    inf_dir
        .child("echo.inf")
        .write_str("[Version]\nSignature = \"$Windows NT$\"\nKmdfLibraryVersion=1.33\nDriverVer=07/01/2024,2.3.4.5\n")
        .unwrap();

    let pkg = detect_driver_type(driver_dir.path(), None).unwrap();
    assert!(detect_samples_repository(driver_dir.path()));
    assert_eq!(pkg.repository, RepositoryType::WindowsRustDriverSamples);
    assert_eq!(pkg.driver_type, DriverType::Kmdf);
    assert_eq!(
        pkg.inf_path.as_ref().unwrap(),
        &inf_dir.child("echo.inf").path().to_path_buf()
    );
    assert_eq!(pkg.version.as_deref(), Some("2.3.4.5"));
}

#[test]
fn samples_build_output_prefers_wdk_layout() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let package = DriverPackage::new(
        root.clone(),
        RepositoryType::WindowsRustDriverSamples,
        DriverType::Kmdf,
        None,
        None,
    );
    let wdk_dir = root.join("target").join("wdk").join("x64").join("Release");
    std::fs::create_dir_all(&wdk_dir).unwrap();
    let build = package.build_output_dir();
    assert_eq!(build, wdk_dir);
}
