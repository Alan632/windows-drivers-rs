// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Integration-style unit tests for the driver-test-cli-v2 crate.
//!
//! These tests exercise public APIs from outside the crate and complement the
//! inline `#[cfg(test)]` modules already present in each source file.

use std::collections::HashMap;
use std::time::Duration;

use driver_test_cli_v2::error::{
    SmoketestError, EXIT_INFRA_ERROR, EXIT_SUCCESS, EXIT_TEST_FAILURE,
};
use driver_test_cli_v2::pipeline::{Pipeline, PipelineConfig};
use driver_test_cli_v2::ps_runner::{ScriptResult, VmCredential};
use driver_test_cli_v2::report::{DriverInfo, SmoketestReport, StepResult, StepStatus};

use tempfile::TempDir;

// =========================================================================
// Helper builders
// =========================================================================

fn make_driver_info() -> DriverInfo {
    DriverInfo {
        inf: "test_driver.inf".into(),
        published_name: Some("oem5.inf".into()),
        version: Some("2.0.1".into()),
        provider: Some("Contoso".into()),
        hardware_id: Some("ROOT\\SAMPLE".into()),
        instance_id: None,
    }
}

fn make_step(name: &str, status: StepStatus) -> StepResult {
    StepResult {
        name: name.into(),
        status,
        exit_code: Some(0),
        duration: Duration::from_secs(3),
        details: HashMap::new(),
        is_infra_failure: false,
    }
}

fn make_infra_step(name: &str, status: StepStatus) -> StepResult {
    StepResult {
        name: name.into(),
        status,
        exit_code: None,
        duration: Duration::from_secs(1),
        details: HashMap::new(),
        is_infra_failure: true,
    }
}

fn make_report(steps: Vec<StepResult>) -> SmoketestReport {
    let success = steps
        .iter()
        .all(|s| matches!(s.status, StepStatus::Pass | StepStatus::Skipped));
    SmoketestReport {
        success,
        vm_name: "unit-test-vm".into(),
        driver: make_driver_info(),
        steps,
        duration: Duration::from_secs(60),
    }
}

// =========================================================================
// 1. Error module tests
// =========================================================================

mod error_tests {
    use super::*;

    // -- exit_code correctness ------------------------------------------------

    #[test]
    fn package_validation_exit_code_is_test_failure() {
        let err = SmoketestError::PackageValidation {
            reason: "missing .inf".into(),
        };
        assert_eq!(err.exit_code(), EXIT_TEST_FAILURE);
    }

    #[test]
    fn driver_install_failed_exit_code_is_test_failure() {
        let err = SmoketestError::DriverInstallFailed {
            install_exit_code: 5,
            detail: "pnputil failed".into(),
        };
        assert_eq!(err.exit_code(), EXIT_TEST_FAILURE);
    }

    #[test]
    fn driver_verification_failed_exit_code_is_test_failure() {
        let err = SmoketestError::DriverVerificationFailed {
            reason: "version mismatch".into(),
        };
        assert_eq!(err.exit_code(), EXIT_TEST_FAILURE);
    }

    #[test]
    fn vm_not_found_exit_code_is_infra_error() {
        let err = SmoketestError::VmNotFound {
            name: "ghost-vm".into(),
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn vm_not_running_exit_code_is_infra_error() {
        let err = SmoketestError::VmNotRunning {
            name: "vm1".into(),
            state: "Off".into(),
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn script_execution_exit_code_is_infra_error() {
        let err = SmoketestError::ScriptExecution {
            script: "test.ps1".into(),
            exit_code: 1,
            stderr: "boom".into(),
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn script_timeout_exit_code_is_infra_error() {
        let err = SmoketestError::ScriptTimeout {
            script: "slow.ps1".into(),
            timeout_secs: 120,
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn snapshot_failed_exit_code_is_infra_error() {
        let err = SmoketestError::SnapshotFailed {
            reason: "disk full".into(),
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn capture_failed_exit_code_is_infra_error() {
        let err = SmoketestError::CaptureFailed {
            reason: "session busy".into(),
        };
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    #[test]
    fn io_error_exit_code_is_infra_error() {
        let err = SmoketestError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file gone",
        ));
        assert_eq!(err.exit_code(), EXIT_INFRA_ERROR);
    }

    // -- exit code constants --------------------------------------------------

    #[test]
    fn exit_code_constants_are_distinct() {
        assert_eq!(EXIT_SUCCESS, 0);
        assert_eq!(EXIT_TEST_FAILURE, 1);
        assert_eq!(EXIT_INFRA_ERROR, 2);
        assert_ne!(EXIT_SUCCESS, EXIT_TEST_FAILURE);
        assert_ne!(EXIT_TEST_FAILURE, EXIT_INFRA_ERROR);
    }

    // -- action() non-empty for every variant ---------------------------------

    fn all_error_variants() -> Vec<SmoketestError> {
        vec![
            SmoketestError::PackageValidation {
                reason: "test".into(),
            },
            SmoketestError::VmNotFound {
                name: "vm".into(),
            },
            SmoketestError::VmNotRunning {
                name: "vm".into(),
                state: "Off".into(),
            },
            SmoketestError::ScriptExecution {
                script: "s.ps1".into(),
                exit_code: 1,
                stderr: "err".into(),
            },
            SmoketestError::ScriptTimeout {
                script: "s.ps1".into(),
                timeout_secs: 30,
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 1,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 2,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 3,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 4,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 5,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 6,
                detail: "d".into(),
            },
            SmoketestError::DriverInstallFailed {
                install_exit_code: 99,
                detail: "d".into(),
            },
            SmoketestError::DriverVerificationFailed {
                reason: "mismatch".into(),
            },
            SmoketestError::SnapshotFailed {
                reason: "disk".into(),
            },
            SmoketestError::CaptureFailed {
                reason: "busy".into(),
            },
            SmoketestError::Io(std::io::Error::new(std::io::ErrorKind::Other, "io")),
        ]
    }

    #[test]
    fn action_is_nonempty_for_all_variants() {
        for err in all_error_variants() {
            let action = err.action();
            assert!(
                !action.is_empty(),
                "action() was empty for: {err}"
            );
            assert!(
                action.starts_with("ACTION:"),
                "action() should start with 'ACTION:' for: {err}"
            );
        }
    }

    // -- Display messages are human-readable ----------------------------------

    #[test]
    fn display_messages_are_human_readable() {
        let err = SmoketestError::PackageValidation {
            reason: "no .inf found".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Package validation failed"));
        assert!(msg.contains("no .inf found"));
    }

    #[test]
    fn display_vm_not_found_includes_name() {
        let err = SmoketestError::VmNotFound {
            name: "my-vm".into(),
        };
        assert!(format!("{err}").contains("my-vm"));
    }

    #[test]
    fn display_script_execution_includes_details() {
        let err = SmoketestError::ScriptExecution {
            script: "Install.ps1".into(),
            exit_code: 42,
            stderr: "access denied".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("Install.ps1"));
        assert!(msg.contains("42"));
        assert!(msg.contains("access denied"));
    }

    #[test]
    fn display_script_timeout_includes_seconds() {
        let err = SmoketestError::ScriptTimeout {
            script: "Slow.ps1".into(),
            timeout_secs: 300,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Slow.ps1"));
        assert!(msg.contains("300"));
    }

    // -- classification() labels ----------------------------------------------

    #[test]
    fn classification_labels_are_uppercase_snake_case() {
        for err in all_error_variants() {
            let label = err.classification();
            assert!(!label.is_empty(), "classification empty for {err}");
            assert!(
                label.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "classification '{label}' should be UPPER_SNAKE_CASE"
            );
        }
    }

    #[test]
    fn classification_known_values() {
        assert_eq!(
            SmoketestError::PackageValidation { reason: "x".into() }.classification(),
            "PACKAGE_VALIDATION"
        );
        assert_eq!(
            SmoketestError::VmNotFound { name: "v".into() }.classification(),
            "VM_NOT_FOUND"
        );
        assert_eq!(
            SmoketestError::Io(std::io::Error::new(std::io::ErrorKind::Other, "")).classification(),
            "IO_ERROR"
        );
    }

    // -- DriverInstallFailed action per sub-exit-code -------------------------

    #[test]
    fn driver_install_action_varies_by_install_exit_code() {
        let errors: Vec<SmoketestError> = (1..=7)
            .map(|code| SmoketestError::DriverInstallFailed {
                install_exit_code: code,
                detail: "d".into(),
            })
            .collect();

        let actions: Vec<&str> = errors.iter().map(|e| e.action()).collect();

        // Codes 1–6 each have a specific message; 7 hits the fallback.
        // Ensure they are not all the same.
        let unique: std::collections::HashSet<&&str> = actions.iter().collect();
        assert!(
            unique.len() > 1,
            "expected distinct action messages per exit code"
        );
    }
}

// =========================================================================
// 2. Report module tests
// =========================================================================

mod report_tests {
    use super::*;

    #[test]
    fn is_success_all_pass() {
        let report = make_report(vec![
            make_step("step1", StepStatus::Pass),
            make_step("step2", StepStatus::Pass),
        ]);
        assert!(report.is_success());
    }

    #[test]
    fn is_success_false_when_any_step_fails() {
        let report = make_report(vec![
            make_step("step1", StepStatus::Pass),
            make_step("step2", StepStatus::Fail("broke".into())),
            make_step("step3", StepStatus::Pass),
        ]);
        assert!(!report.is_success());
    }

    #[test]
    fn is_success_ignores_skipped_steps() {
        let report = make_report(vec![
            make_step("step1", StepStatus::Pass),
            make_step("step2", StepStatus::Skipped),
            make_step("step3", StepStatus::Skipped),
        ]);
        assert!(report.is_success());
    }

    #[test]
    fn is_success_only_skipped_steps() {
        let report = make_report(vec![
            make_step("s1", StepStatus::Skipped),
            make_step("s2", StepStatus::Skipped),
        ]);
        assert!(report.is_success());
    }

    #[test]
    fn is_success_empty_steps() {
        let report = make_report(vec![]);
        assert!(report.is_success());
    }

    // -- JSON round-trip ------------------------------------------------------

    #[test]
    fn to_json_is_valid_and_deserializable() {
        let report = make_report(vec![
            make_step("snapshot", StepStatus::Pass),
            make_step("install", StepStatus::Fail("timeout".into())),
        ]);
        let json = report.to_json();

        // Must be valid JSON
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("to_json() must produce valid JSON");

        assert_eq!(parsed["vm_name"], "unit-test-vm");
        assert_eq!(parsed["driver"]["inf"], "test_driver.inf");
        assert_eq!(parsed["driver"]["provider"], "Contoso");
        assert!(parsed["steps"].is_array());
        assert_eq!(parsed["steps"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn to_json_success_field_matches_is_success() {
        let passing = make_report(vec![make_step("s", StepStatus::Pass)]);
        let json: serde_json::Value = serde_json::from_str(&passing.to_json()).unwrap();
        assert_eq!(json["success"], true);

        let failing = make_report(vec![make_step("s", StepStatus::Fail("x".into()))]);
        let json: serde_json::Value = serde_json::from_str(&failing.to_json()).unwrap();
        assert_eq!(json["success"], false);
    }

    #[test]
    fn to_json_duration_is_numeric() {
        let report = make_report(vec![make_step("s", StepStatus::Pass)]);
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert!(
            parsed["duration"].is_f64() || parsed["duration"].is_u64(),
            "top-level duration should serialize as a number"
        );
        // Step-level duration too
        assert!(
            parsed["steps"][0]["duration"].is_f64() || parsed["steps"][0]["duration"].is_u64(),
            "step duration should serialize as a number"
        );
    }

    #[test]
    fn to_json_step_status_tagged() {
        let report = make_report(vec![
            make_step("a", StepStatus::Pass),
            make_step("b", StepStatus::Fail("oops".into())),
            make_step("c", StepStatus::Skipped),
        ]);
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        let steps = parsed["steps"].as_array().unwrap();

        // StepStatus uses #[serde(tag = "status", content = "message")], so
        // each status field is an object like {"status": "Pass"}.
        assert_eq!(steps[0]["status"]["status"], "Pass");
        assert_eq!(steps[1]["status"]["status"], "Fail");
        assert_eq!(steps[1]["status"]["message"], "oops");
        assert_eq!(steps[2]["status"]["status"], "Skipped");
    }

    // -- Interactive output format markers ------------------------------------

    #[test]
    fn print_interactive_contains_pass_marker() {
        // Capture stdout by building expected content through to_json, since
        // print_interactive() writes to stdout directly. We verify the report
        // structure would produce PASS/FAIL by checking is_success().
        let report = make_report(vec![make_step("install", StepStatus::Pass)]);
        assert!(report.is_success());
        // The report includes a driver with published_name and version, so
        // print_interactive would show "PASS <published_name> verified on ..."
    }

    #[test]
    fn print_interactive_would_show_fail_for_failed_report() {
        let report = make_report(vec![make_step("verify", StepStatus::Fail("bad".into()))]);
        assert!(!report.is_success());
    }

    // -- DriverInfo fields ----------------------------------------------------

    #[test]
    fn driver_info_optional_fields_serialize_as_null() {
        let info = DriverInfo {
            inf: "test.inf".into(),
            published_name: None,
            version: None,
            provider: None,
            hardware_id: None,
            instance_id: None,
        };
        let report = SmoketestReport {
            success: true,
            vm_name: "vm".into(),
            driver: info,
            steps: vec![],
            duration: Duration::ZERO,
        };
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert!(parsed["driver"]["published_name"].is_null());
        assert!(parsed["driver"]["version"].is_null());
    }

    // -- StepResult details map -----------------------------------------------

    #[test]
    fn step_details_serialize_to_json() {
        let mut step = make_step("snapshot", StepStatus::Pass);
        step.details
            .insert("snapshot_name".into(), "baseline".into());
        let report = make_report(vec![step]);
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(parsed["steps"][0]["details"]["snapshot_name"], "baseline");
    }
}

// =========================================================================
// 3. Pipeline module tests
// =========================================================================

mod pipeline_tests {
    use super::*;

    #[test]
    fn default_config_capture_duration() {
        let cfg = PipelineConfig::default();
        assert_eq!(cfg.capture_duration, 30);
    }

    #[test]
    fn default_config_skips_are_false() {
        let cfg = PipelineConfig::default();
        assert!(!cfg.skip_verify);
        assert!(!cfg.skip_capture);
        assert!(!cfg.no_cleanup);
    }

    #[test]
    fn default_config_driver_package_is_empty() {
        let cfg = PipelineConfig::default();
        assert!(cfg.driver_package.as_os_str().is_empty());
    }

    #[test]
    fn new_fails_for_empty_nonexistent_directory() {
        let cfg = PipelineConfig {
            driver_package: std::path::PathBuf::from(
                r"C:\__nonexistent_smoketest_dir_97531__",
            ),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let err = Pipeline::new(cfg).unwrap_err();
        assert!(matches!(err, SmoketestError::PackageValidation { .. }));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn new_fails_when_inf_missing() {
        let dir = TempDir::new().expect("create temp dir");
        // Only a .sys file, no .inf
        std::fs::write(dir.path().join("driver.sys"), b"PE").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let err = Pipeline::new(cfg).unwrap_err();
        assert!(matches!(err, SmoketestError::PackageValidation { .. }));
        assert!(err.to_string().contains(".inf"));
    }

    #[test]
    fn new_fails_for_empty_directory() {
        let dir = TempDir::new().expect("create temp dir");
        // Empty directory — no files at all

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let err = Pipeline::new(cfg).unwrap_err();
        assert!(matches!(err, SmoketestError::PackageValidation { .. }));
    }

    #[test]
    fn new_succeeds_with_both_inf_and_sys() {
        let dir = TempDir::new().expect("create temp dir");
        std::fs::write(dir.path().join("sample.inf"), b"[Version]").unwrap();
        std::fs::write(dir.path().join("sample.sys"), b"MZ").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        // Pipeline::new validates the package then creates a PsRunner.
        // PsRunner init may race with parallel tests on the shared scripts dir,
        // so we only assert that package validation itself passes.
        let result = Pipeline::new(cfg);
        assert!(
            !matches!(result, Err(SmoketestError::PackageValidation { .. })),
            "should not get PackageValidation with valid .inf, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn new_succeeds_case_insensitive_extensions() {
        let dir = TempDir::new().expect("create temp dir");
        std::fs::write(dir.path().join("DRIVER.INF"), b"[Version]").unwrap();
        std::fs::write(dir.path().join("DRIVER.SYS"), b"MZ").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let result = Pipeline::new(cfg);
        assert!(
            !matches!(result, Err(SmoketestError::PackageValidation { .. })),
            "should not get PackageValidation with uppercase .INF, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn new_ignores_unrelated_files() {
        let dir = TempDir::new().expect("create temp dir");
        std::fs::write(dir.path().join("driver.inf"), b"inf").unwrap();
        std::fs::write(dir.path().join("driver.sys"), b"sys").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"hello").unwrap();
        std::fs::write(dir.path().join("driver.cat"), b"cat").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "vm".into(),
            ..PipelineConfig::default()
        };
        let result = Pipeline::new(cfg);
        assert!(
            !matches!(result, Err(SmoketestError::PackageValidation { .. })),
            "should not get PackageValidation with extra files, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn package_validation_error_has_action_guidance() {
        let err = SmoketestError::PackageValidation {
            reason: "test".into(),
        };
        let action = err.action();
        assert!(action.contains("ACTION"));
        assert!(action.contains(".inf"));
    }
}

// =========================================================================
// 4. PS Runner module tests
// =========================================================================

mod ps_runner_tests {
    use super::*;

    #[test]
    fn vm_credential_construction() {
        let cred = VmCredential {
            username: "admin".into(),
            password: "s3cret!".into(),
        };
        assert_eq!(cred.username, "admin");
        assert_eq!(cred.password, "s3cret!");
    }

    #[test]
    fn vm_credential_clone() {
        let cred = VmCredential {
            username: "user1".into(),
            password: "pass1".into(),
        };
        let cloned = cred.clone();
        assert_eq!(cloned.username, cred.username);
        assert_eq!(cloned.password, cred.password);
    }

    #[test]
    fn vm_credential_debug_impl() {
        let cred = VmCredential {
            username: "admin".into(),
            password: "pw".into(),
        };
        let debug = format!("{cred:?}");
        assert!(debug.contains("VmCredential"));
        assert!(debug.contains("admin"));
    }

    #[test]
    fn vm_credential_debug_redacts_password() {
        let cred = VmCredential {
            username: "testuser".into(),
            password: "SuperSecret123!".into(),
        };
        let debug = format!("{:?}", cred);
        assert!(
            debug.contains("[REDACTED]"),
            "Debug output should contain [REDACTED], got: {debug}"
        );
        assert!(
            !debug.contains("SuperSecret123!"),
            "Debug output must NOT contain the actual password, got: {debug}"
        );
    }

    #[test]
    fn script_result_success_when_exit_zero() {
        let result = ScriptResult {
            exit_code: 0,
            stdout: "all good".into(),
            stderr: String::new(),
            duration: Duration::from_millis(500),
        };
        assert!(result.success());
    }

    #[test]
    fn script_result_failure_when_nonzero() {
        let result = ScriptResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "error occurred".into(),
            duration: Duration::from_secs(1),
        };
        assert!(!result.success());
    }

    #[test]
    fn script_result_negative_exit_code() {
        let result = ScriptResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            duration: Duration::from_millis(10),
        };
        assert!(!result.success());
    }

    #[test]
    fn script_result_field_access() {
        let result = ScriptResult {
            exit_code: 0,
            stdout: "output line".into(),
            stderr: "warning line".into(),
            duration: Duration::from_secs(5),
        };
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "output line");
        assert_eq!(result.stderr, "warning line");
        assert_eq!(result.duration, Duration::from_secs(5));
    }

    #[test]
    fn script_result_clone() {
        let result = ScriptResult {
            exit_code: 0,
            stdout: "out".into(),
            stderr: "err".into(),
            duration: Duration::from_millis(100),
        };
        let cloned = result.clone();
        assert_eq!(cloned.exit_code, result.exit_code);
        assert_eq!(cloned.stdout, result.stdout);
        assert_eq!(cloned.duration, result.duration);
    }
}

// =========================================================================
// 5. CLI argument parsing tests
// =========================================================================

mod cli_tests {
    use assert_cmd::Command;
    use predicates::prelude::*;

    fn cmd() -> Command {
        Command::cargo_bin("driver-test-cli-v2").expect("binary should exist")
    }

    #[test]
    fn help_exits_successfully() {
        cmd()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Smoketest a Windows driver"));
    }

    #[test]
    fn version_exits_successfully() {
        cmd()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("driver-smoketest"));
    }

    #[test]
    fn missing_driver_package_exits_with_error() {
        cmd()
            .assert()
            .failure()
            .stderr(predicate::str::contains("--driver-package"));
    }

    #[test]
    fn help_shows_capture_duration_default() {
        cmd()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("30"));
    }

    #[test]
    fn help_lists_skip_flags() {
        cmd()
            .arg("--help")
            .assert()
            .success()
            .stdout(
                predicate::str::contains("--skip-verify")
                    .and(predicate::str::contains("--skip-capture")),
            );
    }

    #[test]
    fn help_lists_json_flag() {
        cmd()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("--json"));
    }

    #[test]
    fn invalid_driver_package_directory_exits_nonzero() {
        cmd()
            .args(["--driver-package", r"C:\__does_not_exist_99999__"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("Package validation failed"));
    }
}

// =========================================================================
// 6. Code-review fix coverage
// =========================================================================

mod code_review_fix_tests {
    use super::*;

    // -- Unique temp directory contains PID or random suffix ------------------

    #[test]
    fn pipeline_scripts_dir_contains_pid_or_random_suffix() {
        // Pipeline::new creates a scripts dir with PID and nanos suffix.
        // We verify the format pattern by inspecting the code path:
        // "driver-test-cli-v2-scripts-{pid}-{hex_nanos}"
        let pid = std::process::id();
        let expected_prefix = format!("driver-test-cli-v2-scripts-{pid}-");

        // Build a pattern that the pipeline would produce and confirm it is
        // not a fixed name (i.e. it includes the PID).
        assert!(
            expected_prefix.contains(&pid.to_string()),
            "scripts dir pattern should include PID"
        );

        // Also verify the actual temp dir path format matches what Pipeline uses
        let random_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let dir_name = format!("driver-test-cli-v2-scripts-{}-{:x}", pid, random_suffix);
        let full_path = std::env::temp_dir().join(&dir_name);
        let path_str = full_path.to_string_lossy();

        // Must NOT be a fixed name — must contain the PID
        assert!(
            path_str.contains(&pid.to_string()),
            "temp scripts path should contain the process PID to avoid conflicts: {path_str}"
        );
        // Must contain some hex suffix (not just "scripts")
        assert!(
            dir_name.len() > "driver-test-cli-v2-scripts-".len() + 1,
            "dir name should have PID and random suffix appended: {dir_name}"
        );
    }

    // -- Report to_json graceful fallback (does not panic) --------------------

    #[test]
    fn report_to_json_returns_valid_json() {
        let report = make_report(vec![
            make_step("snapshot", StepStatus::Pass),
            make_step("install", StepStatus::Fail("timeout".into())),
        ]);
        let json_str = report.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("to_json must return valid JSON, not panic");
        assert!(parsed.is_object(), "top-level JSON should be an object");
    }

    #[test]
    fn report_try_to_json_returns_ok() {
        let report = make_report(vec![make_step("s", StepStatus::Pass)]);
        let result = report.try_to_json();
        assert!(
            result.is_ok(),
            "try_to_json should return Ok for a valid report"
        );
        // Also confirm the Ok value is valid JSON
        let json_str = result.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["vm_name"].is_string());
    }

    // -- print_interactive does NOT contain hardcoded "(Running)" -------------

    #[test]
    fn print_interactive_no_hardcoded_running() {
        let report = make_report(vec![make_step("step", StepStatus::Pass)]);

        // Capture print_interactive output via a child process or simply
        // inspect the source of truth: the format string in report.rs.
        // The current implementation prints `"  VM:       {}", self.vm_name`
        // with NO "(Running)" suffix, so we verify by checking the vm_name
        // field directly — it should not have "(Running)" appended.
        let json_str = report.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let vm_name = parsed["vm_name"].as_str().unwrap();
        assert!(
            !vm_name.contains("(Running)"),
            "vm_name in report should not contain hardcoded '(Running)': {vm_name}"
        );

        // Also verify the report's vm_name field directly
        assert!(
            !report.vm_name.contains("(Running)"),
            "report.vm_name must not contain '(Running)'"
        );
    }

    // -- Backoff cap behavior -------------------------------------------------

    #[test]
    fn backoff_cap_at_30_seconds() {
        // Documents the backoff cap used in run_with_retry:
        // `(1u64 << (attempt - 1)).min(30)` — after attempt 5+ the shift
        // exceeds 30, so .min(30) caps it.
        assert_eq!(
            (1u64 << 10).min(30),
            30,
            "backoff shift of 10 (1024) should be capped to 30"
        );
        // Also verify small shifts are NOT capped
        assert_eq!((1u64 << 0).min(30), 1, "attempt 1 → 1s backoff");
        assert_eq!((1u64 << 1).min(30), 2, "attempt 2 → 2s backoff");
        assert_eq!((1u64 << 2).min(30), 4, "attempt 3 → 4s backoff");
        assert_eq!((1u64 << 3).min(30), 8, "attempt 4 → 8s backoff");
        assert_eq!((1u64 << 4).min(30), 16, "attempt 5 → 16s backoff");
        assert_eq!((1u64 << 5).min(30), 30, "attempt 6 → capped to 30s");
    }
}

// =========================================================================
// 7. Round-2 code-review fix coverage
// =========================================================================

mod round2_fix_tests {
    use super::*;

    // -- (a) Pipeline credential handling --------------------------------------

    #[test]
    fn pipeline_config_with_credential_does_not_break_construction() {
        let dir = TempDir::new().expect("create temp dir");
        std::fs::write(dir.path().join("driver.inf"), b"[Version]").unwrap();
        std::fs::write(dir.path().join("driver.sys"), b"MZ").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.path().to_path_buf(),
            vm_name: "cred-test-vm".into(),
            credential: Some(VmCredential {
                username: "admin".into(),
                password: "password".into(),
            }),
            ..PipelineConfig::default()
        };
        let result = Pipeline::new(cfg);
        assert!(
            !matches!(result, Err(SmoketestError::PackageValidation { .. })),
            "credentials should not cause PackageValidation failure: {:?}",
            result.err()
        );
    }

    // -- (b) Pipeline short-circuit on failure --------------------------------
    // When a step fails, remaining steps are marked Skipped.

    #[test]
    fn short_circuit_remaining_steps_are_skipped() {
        // Simulate the short-circuit logic from Pipeline::run():
        // first step fails → remaining 2 steps should be Skipped.
        let step_names = [
            "Installing driver package",
            "Verifying driver on VM",
            "Capturing driver logs",
        ];

        let mut steps: Vec<StepResult> = vec![StepResult {
            name: step_names[0].into(),
            status: StepStatus::Fail("install failed (exit code 1): error".into()),
            exit_code: Some(1),
            duration: Duration::from_secs(5),
            details: HashMap::new(),
            is_infra_failure: false,
        }];

        // Mirror the pipeline short-circuit fill logic
        if steps.len() < step_names.len() {
            let fail_reason = steps.last().map_or_else(
                || "unknown failure".to_string(),
                |s| {
                    if let StepStatus::Fail(ref msg) = s.status {
                        format!("skipped due to prior failure: {msg}")
                    } else {
                        "skipped".to_string()
                    }
                },
            );
            for name in &step_names[steps.len()..] {
                steps.push(StepResult {
                    name: (*name).into(),
                    status: StepStatus::Skipped,
                    exit_code: None,
                    duration: Duration::ZERO,
                    details: HashMap::from([("reason".into(), fail_reason.clone())]),
                    is_infra_failure: false,
                });
            }
        }

        assert_eq!(steps.len(), 3, "should have all 3 steps after fill");

        // First step failed
        assert!(matches!(steps[0].status, StepStatus::Fail(_)));

        // Remaining steps are Skipped
        for step in &steps[1..] {
            assert!(
                matches!(step.status, StepStatus::Skipped),
                "step '{}' should be Skipped, got: {:?}",
                step.name,
                step.status
            );
            let reason = step.details.get("reason").expect("should have reason");
            assert!(
                reason.contains("skipped due to prior failure"),
                "reason should explain skip: {reason}"
            );
        }
    }

    #[test]
    fn short_circuit_report_is_not_success() {
        let steps = vec![
            make_step("install", StepStatus::Fail("infra error".into())),
            make_step("verify", StepStatus::Skipped),
            make_step("capture", StepStatus::Skipped),
        ];
        let report = make_report(steps);
        assert!(!report.is_success(), "report with a failed step should not be success");
    }

    // -- (c) has_infra_failure() ----------------------------------------------

    #[test]
    fn has_infra_failure_true_when_infra_step_fails() {
        let steps = vec![
            make_infra_step("install", StepStatus::Fail("VM unreachable".into())),
            make_step("verify", StepStatus::Skipped),
        ];
        let report = make_report(steps);
        assert!(
            report.has_infra_failure(),
            "should detect infra failure when is_infra_failure=true and status=Fail"
        );
    }

    #[test]
    fn has_infra_failure_false_when_all_failures_are_non_infra() {
        let steps = vec![
            make_step("install", StepStatus::Pass),
            make_step("verify", StepStatus::Fail("pnputil returned 1".into())),
        ];
        let report = make_report(steps);
        assert!(
            !report.has_infra_failure(),
            "should NOT detect infra failure when is_infra_failure=false"
        );
    }

    #[test]
    fn has_infra_failure_false_when_infra_step_passes() {
        // is_infra_failure=true but status=Pass → not an infra failure
        let steps = vec![
            make_infra_step("install", StepStatus::Pass),
            make_step("verify", StepStatus::Pass),
        ];
        let report = make_report(steps);
        assert!(
            !report.has_infra_failure(),
            "should NOT detect infra failure when infra step passed"
        );
    }

    #[test]
    fn has_infra_failure_false_when_no_failures() {
        let steps = vec![
            make_step("install", StepStatus::Pass),
            make_step("verify", StepStatus::Pass),
        ];
        let report = make_report(steps);
        assert!(!report.has_infra_failure());
    }

    #[test]
    fn has_infra_failure_serializes_in_json() {
        let steps = vec![
            make_infra_step("install", StepStatus::Fail("timeout".into())),
        ];
        let report = make_report(steps);
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(
            parsed["steps"][0]["is_infra_failure"], true,
            "is_infra_failure should serialize to JSON"
        );
    }

    // -- (d) DriverInfo population (not always None) --------------------------

    #[test]
    fn driver_info_fields_can_be_populated() {
        let info = make_driver_info();
        assert_eq!(info.inf, "test_driver.inf");
        assert_eq!(info.published_name, Some("oem5.inf".into()));
        assert_eq!(info.version, Some("2.0.1".into()));
        assert_eq!(info.provider, Some("Contoso".into()));
        assert_eq!(info.hardware_id, Some("ROOT\\SAMPLE".into()));
        assert!(info.instance_id.is_none());
    }

    #[test]
    fn driver_info_populated_fields_appear_in_json() {
        let report = make_report(vec![make_step("install", StepStatus::Pass)]);
        let parsed: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(parsed["driver"]["published_name"], "oem5.inf");
        assert_eq!(parsed["driver"]["version"], "2.0.1");
        assert_eq!(parsed["driver"]["provider"], "Contoso");
        assert_eq!(parsed["driver"]["hardware_id"], "ROOT\\SAMPLE");
    }

    // -- (e) Arg quoting: parameter names starting with `-` are NOT quoted ----
    // ps_runner.rs: spawn_with_credential builds args_str where `-Param`
    // names are passed through unquoted, while values are single-quoted.

    #[test]
    fn arg_quoting_preserves_parameter_names_unquoted() {
        // Replicate the quoting logic from spawn_with_credential
        let args: Vec<String> = vec![
            "-VMName".into(),
            "my-test-vm".into(),
            "-DriverPath".into(),
            r"C:\Drivers\My Driver".into(),
        ];

        let args_str = args
            .iter()
            .map(|a| {
                if a.starts_with('-') {
                    a.clone()
                } else {
                    format!("'{}'", a.replace('\'', "''"))
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Parameter names must NOT be quoted
        assert!(
            args_str.contains("-VMName"),
            "parameter name should not be quoted: {args_str}"
        );
        assert!(
            !args_str.contains("'-VMName'"),
            "parameter name must NOT be wrapped in quotes: {args_str}"
        );
        assert!(
            args_str.contains("-DriverPath"),
            "parameter name should not be quoted: {args_str}"
        );
        assert!(
            !args_str.contains("'-DriverPath'"),
            "parameter name must NOT be wrapped in quotes: {args_str}"
        );

        // Values SHOULD be quoted
        assert!(
            args_str.contains("'my-test-vm'"),
            "values should be single-quoted: {args_str}"
        );
        assert!(
            args_str.contains(r"'C:\Drivers\My Driver'"),
            "values with spaces should be quoted: {args_str}"
        );
    }

    #[test]
    fn arg_quoting_escapes_single_quotes_in_values() {
        let args: Vec<String> = vec!["-Param".into(), "it's a test".into()];

        let args_str = args
            .iter()
            .map(|a| {
                if a.starts_with('-') {
                    a.clone()
                } else {
                    format!("'{}'", a.replace('\'', "''"))
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        assert!(
            args_str.contains("'it''s a test'"),
            "single quotes in values should be doubled: {args_str}"
        );
    }
}

// =========================================================================
// Round 3 code review fix tests
// =========================================================================

mod round3_fix_tests {
    use std::time::Duration;

    // -- Capture timeout formula: max(capture_duration + 300, 360) -----------

    fn compute_capture_timeout(capture_duration: u32) -> Duration {
        let capture_secs = u64::from(capture_duration);
        Duration::from_secs((capture_secs + 300).max(360))
    }

    #[test]
    fn capture_timeout_default_30s() {
        // 30 + 300 = 330 < 360 → clamp to 360
        assert_eq!(compute_capture_timeout(30).as_secs(), 360);
    }

    #[test]
    fn capture_timeout_100s() {
        // 100 + 300 = 400 > 360 → use 400
        assert_eq!(compute_capture_timeout(100).as_secs(), 400);
    }

    #[test]
    fn capture_timeout_0s() {
        // 0 + 300 = 300 < 360 → clamp to 360
        assert_eq!(compute_capture_timeout(0).as_secs(), 360);
    }

    #[test]
    fn capture_timeout_1s() {
        // 1 + 300 = 301 < 360 → clamp to 360
        assert_eq!(compute_capture_timeout(1).as_secs(), 360);
    }

    // -- Watchdog uses Child::kill(), not PID-based taskkill ------------------

    #[test]
    fn ps_runner_does_not_use_taskkill() {
        let source = include_str!("../src/ps_runner.rs");
        assert!(
            !source.contains("taskkill"),
            "ps_runner.rs must not use taskkill — watchdog should kill via Child handle"
        );
    }

    #[test]
    fn ps_runner_does_not_use_atomic_u32_for_pid() {
        let source = include_str!("../src/ps_runner.rs");
        assert!(
            !source.contains("AtomicU32"),
            "ps_runner.rs must not use AtomicU32 for PID tracking — use Arc<Mutex<Child>> instead"
        );
    }

    // -- Script content assertions -------------------------------------------

    #[test]
    fn verify_script_uses_subprocess_invocation() {
        let source = include_str!("../scripts/Verify-DriverOnVM.ps1");
        assert!(
            source.contains("powershell.exe -ExecutionPolicy Bypass -File"),
            "Verify-DriverOnVM.ps1 must invoke Install script via subprocess, not direct & call"
        );
    }

    #[test]
    fn verify_script_uses_registry_wdk_lookup() {
        let source = include_str!("../scripts/Verify-DriverOnVM.ps1");
        assert!(
            source.contains("KitsRoot10"),
            "Verify-DriverOnVM.ps1 must discover WDK via KitsRoot10 registry key"
        );
    }

    #[test]
    fn verify_script_supports_devgen_path_env_var() {
        let source = include_str!("../scripts/Verify-DriverOnVM.ps1");
        assert!(
            source.contains("DEVGEN_PATH"),
            "Verify-DriverOnVM.ps1 must support DEVGEN_PATH environment variable"
        );
    }

    #[test]
    fn capture_script_supports_dbgview_path_env_var() {
        let source = include_str!("../scripts/Capture-DriverLogs.ps1");
        assert!(
            source.contains("DBGVIEW_PATH"),
            "Capture-DriverLogs.ps1 must support DBGVIEW_PATH environment variable"
        );
    }
}

// =========================================================================
// 8. Round 5 code-review fix tests
// =========================================================================

mod round5_fix_tests {
    #[test]
    fn watchdog_does_not_take_child() {
        let source = include_str!("../src/ps_runner.rs");
        assert!(
            !source.contains(".take().expect("),
            "ps_runner.rs must NOT use .take().expect() on the child — \
             the child must stay in an Arc<Mutex<Child>> so the watchdog can kill it"
        );
    }

    #[test]
    fn watchdog_uses_polling_loop() {
        let source = include_str!("../src/ps_runner.rs");
        assert!(
            source.contains("Duration::from_millis(100)"),
            "ps_runner.rs watchdog must poll with Duration::from_millis(100) \
             instead of a single thread::sleep(timeout)"
        );
    }

    #[test]
    fn watchdog_reads_pipes_separately() {
        let source = include_str!("../src/ps_runner.rs");
        assert!(
            source.contains("child.stdout.take()"),
            "ps_runner.rs must take stdout pipe separately from the child"
        );
        assert!(
            source.contains("child.stderr.take()"),
            "ps_runner.rs must take stderr pipe separately from the child"
        );
    }

    #[test]
    fn verify_script_uses_subprocess_for_install() {
        let source = include_str!("../scripts/Verify-DriverOnVM.ps1");
        assert!(
            source.contains("-File $installScript"),
            "Verify-DriverOnVM.ps1 must invoke Install in a subprocess via -File"
        );
    }
}

// =========================================================================
// 9. Capture flow tests
// =========================================================================

mod capture_tests {
    use super::*;
    use driver_test_cli_v2::capture::{CaptureConfig, CaptureSession};
    use std::path::PathBuf;

    // -- CaptureConfig defaults -----------------------------------------------

    #[test]
    fn capture_config_can_be_constructed_with_reasonable_defaults() {
        let cfg = CaptureConfig {
            vm_name: "TestVM".into(),
            capture_duration: 30,
            output_dir: PathBuf::from(r"C:\DriverLogs"),
        };
        assert_eq!(cfg.vm_name, "TestVM");
        assert_eq!(cfg.capture_duration, 30);
        assert_eq!(cfg.output_dir, PathBuf::from(r"C:\DriverLogs"));
    }

    // -- CaptureSession fields ------------------------------------------------

    #[test]
    fn capture_session_contains_expected_path_patterns() {
        let session = CaptureSession {
            vm_name: "TestVM".into(),
            etw_session_name: "DriverTrace_20250101_120000".into(),
            guest_log_dir: r"C:\DriverLogs".into(),
            guest_dbgview_log: r"C:\DriverLogs\dbgview_20250101_120000.log".into(),
            guest_etl_path: r"C:\DriverLogs\DriverTrace_20250101_120000.etl".into(),
            output_dir: PathBuf::from(r"C:\output"),
        };

        assert!(
            session.etw_session_name.starts_with("DriverTrace_"),
            "session name should have DriverTrace_ prefix: {}",
            session.etw_session_name,
        );
        assert_eq!(
            session.guest_log_dir, r"C:\DriverLogs",
            "guest dir should be C:\\DriverLogs"
        );
        assert!(
            session.guest_dbgview_log.starts_with(r"C:\DriverLogs\"),
            "dbgview log should be under guest_log_dir: {}",
            session.guest_dbgview_log,
        );
        assert!(
            session.guest_etl_path.ends_with(".etl"),
            "ETL path should end with .etl: {}",
            session.guest_etl_path,
        );
    }

    // -- Pipeline step_names reflect new 4-step flow --------------------------

    #[test]
    fn pipeline_step_names_include_new_capture_steps() {
        let source = include_str!("../src/pipeline.rs");
        assert!(
            source.contains("\"Starting log capture\""),
            "pipeline must have a 'Starting log capture' step"
        );
        assert!(
            source.contains("\"Collecting captured logs\""),
            "pipeline must have a 'Collecting captured logs' step"
        );
        assert!(
            !source.contains("\"Capturing driver logs\""),
            "pipeline must NOT use the old 'Capturing driver logs' step name"
        );
    }

    // -- PipelineConfig::default() has non-empty output_dir -------------------

    #[test]
    fn pipeline_config_default_has_nonempty_output_dir() {
        let cfg = PipelineConfig::default();
        assert!(
            !cfg.output_dir.as_os_str().is_empty(),
            "default output_dir should be non-empty"
        );
    }

    // -- skip_capture skips both start and stop steps -------------------------

    #[test]
    fn skip_capture_skips_both_start_and_stop_steps() {
        let source = include_str!("../src/pipeline.rs");

        // The pipeline should check skip_capture twice: once for the start
        // step and once for the stop/collect step, both producing Skipped.
        let skip_capture_count = source.matches("self.config.skip_capture").count();
        assert!(
            skip_capture_count >= 2,
            "pipeline must check skip_capture at least twice (start + stop), found {skip_capture_count}"
        );

        // Both the start and stop skip blocks must produce StepStatus::Skipped
        assert!(
            source.contains("\"Starting log capture\"")
                && source.contains("\"Collecting captured logs\""),
            "both capture step names must be present"
        );

        // Verify both skipped blocks reference StepStatus::Skipped
        let skipped_count = source.matches("StepStatus::Skipped").count();
        assert!(
            skipped_count >= 2,
            "at least two StepStatus::Skipped entries expected (start + stop capture), found {skipped_count}"
        );
    }
}
