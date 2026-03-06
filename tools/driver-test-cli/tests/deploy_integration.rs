use driver_test_cli::cli::DeployCommand;

#[test]
fn deploy_execute_mock_success() {
    // Force mock deployer path via env var, exercising injection and JSON output path.
    std::env::set_var("DRIVER_TEST_CLI_MOCK", "1");
    let cmd = DeployCommand {
        vm_name: Some("vm1".into()),
        inf: "ignored.inf".into(),
        cert: None,
        expected_version: Some("1.2.3.4".into()),
        wmi: false,
        capture_output: false,
    };
    let result = cmd.run(Some("vm1"), true);
    assert!(result.is_ok(), "mock deploy should succeed");
    std::env::remove_var("DRIVER_TEST_CLI_MOCK");
}
