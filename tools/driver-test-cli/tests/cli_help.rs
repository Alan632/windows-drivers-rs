use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn help_includes_about() {
    let mut cmd = Command::cargo_bin("driver-test-cli").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("Automate Windows driver"));
}
