use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after Unix epoch")
            .as_nanos();
        let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rspyts-cli-build-integration-{}-{nonce}-{counter}",
            std::process::id(),
        ));
        std::fs::create_dir_all(root.join("rust/src")).expect("create fixture directories");

        std::fs::write(
            root.join("rust/Cargo.toml"),
            r#"[package]
name = "fixture-package"
version = "0.1.0"
edition = "2024"

[lib]
name = "actual_bridge"
crate-type = ["rlib", "cdylib"]
"#,
        )
        .expect("write fixture Cargo.toml");
        std::fs::write(
            root.join("rust/src/lib.rs"),
            r##"const MANIFEST: &[u8] = br#"{"abi":"3.0","crateName":"fixture-package","crateVersion":"0.1.0","types":[],"constants":[],"functions":[],"classes":[]}"#;

#[unsafe(no_mangle)]
pub extern "C" fn rspyts_abi_version() -> u32 {
    3
}

#[unsafe(no_mangle)]
pub extern "C" fn rspyts_manifest() -> *mut u8 {
    let mut envelope = Vec::with_capacity(12 + MANIFEST.len());
    envelope.extend_from_slice(&[0, 0, 0, 0]);
    envelope.extend_from_slice(&(MANIFEST.len() as u32).to_le_bytes());
    envelope.extend_from_slice(&0_u32.to_le_bytes());
    envelope.extend_from_slice(MANIFEST);
    Box::into_raw(envelope.into_boxed_slice()).cast::<u8>()
}

/// # Safety
/// `ptr` and `len` must be the exact allocation returned by `rspyts_manifest`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rspyts_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
        }
    }
}
"##,
        )
        .expect("write fixture source");
        std::fs::write(
            root.join("rspyts.toml"),
            r#"[crate]
path = "rust"

[python]
out = "python/src/fixture_package/_generated"
"#,
        )
        .expect("write fixture rspyts.toml");
        Self { root }
    }

    fn command(&self, subcommand: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rspyts"));
        command
            .current_dir(&self.root)
            .arg(subcommand)
            .arg("--config")
            .arg(self.root.join("rspyts.toml"))
            // Explicit CLI `--target <rustc-host>` must override even an
            // unusable ambient Cargo target rather than mislabeling output.
            .env("CARGO_BUILD_TARGET", "not-a-real-rust-target")
            .env("CARGO_TARGET_DIR", self.root.join("cargo-target"));
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn build_is_the_only_command_that_stages_artifacts() {
    let fixture = Fixture::new();
    let generated = fixture.command("generate").output().expect("run generate");
    assert!(
        generated.status.success(),
        "generate failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&generated.stdout),
        String::from_utf8_lossy(&generated.stderr)
    );

    let artifact = fixture
        .root
        .join("python/src/fixture_package/_generated/lib")
        .join(platform_filename("fixture-package"));
    assert!(
        !artifact.exists(),
        "generate must not stage a native library"
    );

    let output = fixture
        .command("build")
        .args(["--output-format", "json"])
        .output()
        .expect("run JSON build");
    assert!(
        output.status.success(),
        "build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // `from_slice` accepting the complete byte slice proves stdout contains
    // no progress lines before or after the single JSON document.
    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout is clean JSON");
    assert_eq!(report["formatVersion"], 1);
    assert_eq!(report["crate"]["name"], "fixture-package");
    assert_eq!(
        report["python"],
        serde_json::json!({"out": fixture.root.join("python/src/fixture_package/_generated")})
    );

    let artifacts = report["artifacts"].as_array().expect("artifact array");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["kind"], "native");
    assert_ne!(artifacts[0]["target"], "not-a-real-rust-target");

    let reported_artifact = PathBuf::from(
        artifacts[0]["path"]
            .as_str()
            .expect("artifact path is a string"),
    );
    assert!(reported_artifact.is_absolute());
    assert!(reported_artifact.is_file());
    assert_eq!(reported_artifact, artifact);
    assert!(
        !reported_artifact
            .to_string_lossy()
            .contains("actual_bridge")
    );
    assert!(
        !reported_artifact
            .to_string_lossy()
            .contains("not-a-real-rust-target")
    );

    let generated_loader = std::fs::read_to_string(
        fixture
            .root
            .join("python/src/fixture_package/_generated/library.py"),
    )
    .expect("read generated loader");
    assert!(generated_loader.contains("        \"lib\","));

    std::fs::remove_file(&artifact).expect("remove source-staged artifact");
    let package_dir = fixture.root.join("package-input");
    let package_output = fixture
        .command("build")
        .args([
            "--out-dir",
            package_dir.to_str().expect("UTF-8 package path"),
            "--output-format",
            "json",
        ])
        .output()
        .expect("run packaging-safe JSON build");
    assert!(
        package_output.status.success(),
        "packaging-safe build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&package_output.stdout),
        String::from_utf8_lossy(&package_output.stderr)
    );
    let package_report: Value =
        serde_json::from_slice(&package_output.stdout).expect("packaging stdout is clean JSON");
    let package_artifact = PathBuf::from(
        package_report["artifacts"][0]["path"]
            .as_str()
            .expect("packaging artifact path is a string"),
    );
    assert_eq!(
        package_artifact,
        package_dir.join(platform_filename("fixture-package"))
    );
    assert!(package_artifact.is_file());
    assert!(
        !artifact.exists(),
        "an explicit output directory must not recreate the package artifact"
    );

    let check_output = fixture
        .command("check")
        .output()
        .expect("run read-only check");
    assert!(
        check_output.status.success(),
        "packaging-safe check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check_output.stdout),
        String::from_utf8_lossy(&check_output.stderr)
    );
    assert!(
        package_artifact.is_file(),
        "check must not remove artifacts"
    );
    assert!(!artifact.exists(), "check must not stage a native library");

    let manifest_output = fixture.command("manifest").output().expect("run manifest");
    assert!(
        manifest_output.status.success(),
        "packaging-safe manifest failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&manifest_output.stdout),
        String::from_utf8_lossy(&manifest_output.stderr)
    );
    let manifest: Value =
        serde_json::from_slice(&manifest_output.stdout).expect("manifest stdout is clean JSON");
    assert_eq!(manifest["crateName"], "fixture-package");
    assert!(
        package_artifact.is_file(),
        "manifest must not remove artifacts"
    );
    assert!(
        !artifact.exists(),
        "manifest must not stage a native library"
    );

    let host = rustc_host();
    let target_only_output = fixture
        .command("build")
        .args(["--target", &host, "--output-format", "json"])
        .output()
        .expect("run target-only JSON build");
    assert!(
        target_only_output.status.success(),
        "target-only build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&target_only_output.stdout),
        String::from_utf8_lossy(&target_only_output.stderr)
    );
    let target_only_report: Value = serde_json::from_slice(&target_only_output.stdout)
        .expect("target-only stdout is clean JSON");
    assert_eq!(target_only_report["artifacts"][0]["kind"], "target");
    assert_eq!(target_only_report["artifacts"][0]["target"], host);
    assert!(
        !artifact.exists(),
        "an explicit target build must not implicitly add the host"
    );

    let deduplicated_dir = fixture.root.join("deduplicated-host");
    let deduplicated_output = fixture
        .command("build")
        .args([
            "--target",
            "host",
            "--target",
            &host,
            "--out-dir",
            deduplicated_dir.to_str().expect("UTF-8 output path"),
            "--output-format",
            "json",
        ])
        .output()
        .expect("run deduplicated host build");
    assert!(
        deduplicated_output.status.success(),
        "deduplicated host build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&deduplicated_output.stdout),
        String::from_utf8_lossy(&deduplicated_output.stderr)
    );
    let deduplicated_report: Value = serde_json::from_slice(&deduplicated_output.stdout)
        .expect("deduplicated host stdout is clean JSON");
    let deduplicated_artifacts = deduplicated_report["artifacts"]
        .as_array()
        .expect("deduplicated artifact array");
    assert_eq!(deduplicated_artifacts.len(), 1);
    assert_eq!(deduplicated_artifacts[0]["kind"], "native");
}

#[test]
fn check_is_read_only_for_clean_and_drifted_sources() {
    let fixture = Fixture::new();
    let generated = fixture.command("generate").output().expect("run generate");
    assert!(
        generated.status.success(),
        "generate failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&generated.stdout),
        String::from_utf8_lossy(&generated.stderr)
    );

    let artifact = fixture
        .root
        .join("python/src/fixture_package/_generated/lib")
        .join(platform_filename("fixture-package"));
    let generated_file = fixture
        .root
        .join("python/src/fixture_package/_generated/functions.py");
    let original_generated =
        std::fs::read_to_string(&generated_file).expect("read generated source");
    const LAST_KNOWN_GOOD: &[u8] = b"previously-staged-native-library";
    std::fs::create_dir_all(artifact.parent().expect("artifact parent"))
        .expect("create artifact directory");
    std::fs::write(&artifact, LAST_KNOWN_GOOD).expect("seed prior staged library");

    let regenerated = fixture
        .command("generate")
        .output()
        .expect("run generate again");
    assert!(regenerated.status.success());
    assert_eq!(
        std::fs::read(&artifact).expect("read artifact after generate"),
        LAST_KNOWN_GOOD,
        "generate writes source only and must not stage artifacts"
    );
    std::fs::write(
        &generated_file,
        format!("{original_generated}\n# local drift\n"),
    )
    .expect("introduce generated-source drift");

    let drifted = fixture
        .command("check")
        .output()
        .expect("run drifted check");
    assert_eq!(
        drifted.status.code(),
        Some(1),
        "drifted check had unexpected status:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&drifted.stdout),
        String::from_utf8_lossy(&drifted.stderr)
    );
    assert!(String::from_utf8_lossy(&drifted.stderr).contains("generated code is out of date"));
    assert_eq!(
        std::fs::read(&artifact).expect("read staged library after failed check"),
        LAST_KNOWN_GOOD,
        "a drifted check must leave artifacts untouched"
    );

    std::fs::write(&generated_file, original_generated).expect("restore generated source");
    let clean = fixture.command("check").output().expect("run clean check");
    assert!(
        clean.status.success(),
        "clean check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&clean.stdout),
        String::from_utf8_lossy(&clean.stderr)
    );
    assert_eq!(
        std::fs::read(&artifact).expect("read native library after clean check"),
        LAST_KNOWN_GOOD,
        "a successful check must also leave artifacts untouched"
    );
}

#[test]
fn invalid_manifest_check_leaves_previous_library_untouched() {
    let fixture = Fixture::new();
    let generated = fixture.command("generate").output().expect("run generate");
    assert!(
        generated.status.success(),
        "generate failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&generated.stdout),
        String::from_utf8_lossy(&generated.stderr)
    );

    let artifact = fixture
        .root
        .join("python/src/fixture_package/_generated/lib")
        .join(platform_filename("fixture-package"));
    const LAST_KNOWN_GOOD: &[u8] = b"previously-staged-native-library";
    std::fs::create_dir_all(artifact.parent().expect("artifact parent"))
        .expect("create artifact directory");
    std::fs::write(&artifact, LAST_KNOWN_GOOD).expect("seed prior staged library");

    let source_path = fixture.root.join("rust/src/lib.rs");
    let source = std::fs::read_to_string(&source_path).expect("read fixture Rust source");
    assert!(source.contains(r#""abi":"3.0""#));
    std::fs::write(
        &source_path,
        source.replace(r#""abi":"3.0""#, r#""abi":"3.1""#),
    )
    .expect("write unsupported manifest ABI");

    let invalid = fixture
        .command("check")
        .output()
        .expect("run invalid-manifest check");
    assert_eq!(
        invalid.status.code(),
        Some(3),
        "invalid-manifest check had unexpected status:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("unsupported manifest ABI `3.1`"));
    assert_eq!(
        std::fs::read(&artifact).expect("read staged library after invalid manifest"),
        LAST_KNOWN_GOOD,
        "an invalid manifest must leave the previous staged library untouched"
    );
}

#[test]
fn invalid_manifest_generate_and_manifest_leave_previous_library_untouched() {
    for subcommand in ["generate", "manifest"] {
        let fixture = Fixture::new();
        let generated = fixture.command("generate").output().expect("run generate");
        assert!(
            generated.status.success(),
            "initial generate failed for {subcommand} case:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&generated.stdout),
            String::from_utf8_lossy(&generated.stderr)
        );

        let artifact = fixture
            .root
            .join("python/src/fixture_package/_generated/lib")
            .join(platform_filename("fixture-package"));
        const LAST_KNOWN_GOOD: &[u8] = b"previously-staged-native-library";
        std::fs::create_dir_all(artifact.parent().expect("artifact parent"))
            .expect("create artifact directory");
        std::fs::write(&artifact, LAST_KNOWN_GOOD).expect("seed prior staged library");

        let source_path = fixture.root.join("rust/src/lib.rs");
        let source = std::fs::read_to_string(&source_path).expect("read fixture Rust source");
        assert!(source.contains(r#""abi":"3.0""#));
        std::fs::write(
            &source_path,
            source.replace(r#""abi":"3.0""#, r#""abi":"3.1""#),
        )
        .expect("write unsupported manifest ABI");

        let invalid = fixture
            .command(subcommand)
            .output()
            .unwrap_or_else(|error| panic!("run invalid-manifest {subcommand}: {error}"));
        assert_eq!(
            invalid.status.code(),
            Some(3),
            "invalid-manifest {subcommand} had unexpected status:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&invalid.stdout),
            String::from_utf8_lossy(&invalid.stderr)
        );
        assert!(
            String::from_utf8_lossy(&invalid.stderr).contains("unsupported manifest ABI `3.1`")
        );
        assert_eq!(
            std::fs::read(&artifact).expect("read staged library after invalid manifest"),
            LAST_KNOWN_GOOD,
            "invalid-manifest {subcommand} must leave the previous staged library untouched"
        );
    }
}

fn platform_filename(name: &str) -> String {
    let stem = name.replace('-', "_");
    if cfg!(target_os = "windows") {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

fn rustc_host() -> String {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("run rustc -vV");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("rustc output is UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc output includes host")
        .to_string()
}
