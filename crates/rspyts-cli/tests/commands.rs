use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rspyts-cli-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("rust/src")).unwrap();
    fs::write(
        root.join("rust/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .unwrap();
    fs::write(root.join("rust/src/lib.rs"), "").unwrap();
    root
}

#[test]
fn clean_removes_only_the_project_staging_directory() {
    let root = project("clean");
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".rspyts")).unwrap();
    fs::write(root.join(".rspyts/stale"), "stale").unwrap();
    fs::write(root.join("keep"), "keep").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("clean")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join(".rspyts").exists());
    assert!(root.join("keep").exists());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schemaVersion"], 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_configuration_is_a_hard_error() {
    let root = project("unknown-config");
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\ngenerate = true\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("inspect")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));
    fs::remove_dir_all(root).unwrap();
}
