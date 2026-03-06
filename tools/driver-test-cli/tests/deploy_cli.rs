use assert_cmd::Command;
use predicates::str::{contains, is_match};

// This test exercises the deploy command JSON output on a missing INF path.
#[test]
fn deploy_missing_inf_json_outputs_error() {
    let mut cmd = Command::cargo_bin("driver-test-cli").expect("binary build");
    cmd.arg("deploy")
        .arg("--inf")
        .arg("this/does/not/exist.inf")
        .arg("--json");
    cmd.assert()
        .failure()
        .stderr(contains("Error: io: inf path"))
        .stdout(is_match(r#"\{"success":false,"published_name":null,"version":null,"wmi":null,"error":".*"\}"#).unwrap());
}
