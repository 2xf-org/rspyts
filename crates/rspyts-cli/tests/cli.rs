use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_rspyts")
}

fn assert_project_layout(project: &Path) {
    for path in [
        "Cargo.toml",
        "crates/api/Cargo.toml",
        "crates/api/src/lib.rs",
        "crates/bindings/Cargo.toml",
        "crates/bindings/src/lib.rs",
        "clients/python/hello_world_client/__init__.py",
        "clients/typescript/package.json",
        "clients/typescript/src/index.ts",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }
    assert!(!project.join("clients/python/tests").exists());
}

#[test]
fn explicit_entry_point_captures_output_and_creates_a_project() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let project = directory.path().join("hello-world");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    rspyts_cli::run_with(
        [
            OsString::from("rspyts"),
            OsString::from("init"),
            project.clone().into_os_string(),
        ],
        &mut stdout,
        &mut stderr,
    )
    .expect("init succeeds");

    assert!(stderr.is_empty());
    let report: Value = serde_json::from_slice(&stdout).expect("JSON report");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["project"], project.to_string_lossy().as_ref());
    assert_project_layout(&project);
}

#[test]
fn init_runs_end_to_end_through_the_binary() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let project = directory.path().join("hello-world");

    let output = Command::new(binary())
        .arg("init")
        .arg(&project)
        .output()
        .expect("run rspyts");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["status"], "ok");
    assert_project_layout(&project);
}

#[test]
fn invalid_init_fails_without_leaving_a_partial_project() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let project = directory.path().join("Invalid_name");

    let output = Command::new(binary())
        .arg("init")
        .arg(&project)
        .output()
        .expect("run rspyts");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("must use lower-case letters, numbers, and single hyphens")
    );
    assert!(!project.exists());
}
