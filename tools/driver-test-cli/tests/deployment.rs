#[cfg(test)]
mod tests {
    use driver_test_cli::deploy::{DriverDeployer, PnpDeployer};
    use driver_test_cli::vm::TestVm;
    use std::path::PathBuf;

    // Minimal TestVm stub (align with actual struct fields in vm.rs)
    fn test_vm() -> TestVm {
        TestVm {
            name: "fake".into(),
            state: "Running".into(),
            memory_mb: 0,
            cpus: 1,
        }
    }

    // These tests exercise error paths that do not rely on real PowerShell success.
    // File existence checks happen before PowerShell execution.
    #[test]
    fn certificate_missing_path_errors() {
        let deployer = PnpDeployer::default();
        let vm = test_vm();
        let bad = PathBuf::from("nonexistent.cer");
        let err = deployer.install_certificate(&vm, &bad).unwrap_err();
        assert!(format!("{err}").contains("io"));
    }

    #[test]
    fn inf_missing_path_errors() {
        let deployer = PnpDeployer::default();
        let vm = test_vm();
        let bad = PathBuf::from("missing.inf");
        let err = deployer.install_driver(&vm, &bad).unwrap_err();
        assert!(format!("{err}").contains("io"));
    }
}
