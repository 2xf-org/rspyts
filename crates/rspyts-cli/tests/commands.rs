use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PROJECT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rspyts-cli-{name}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        PROJECT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(root.join("rust/src")).unwrap();
    fs::write(
        root.join("rust/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .unwrap();
    fs::write(root.join("rust/src/lib.rs"), "").unwrap();
    generate_lockfile(&root);
    root
}

fn generate_lockfile(root: &Path) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(root.join("rust/Cargo.toml"))
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clean_removes_only_the_project_output_directory() {
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

#[test]
fn missing_discovery_abi_symbol_reports_exact_version_remediation() {
    let root = project("missing-discovery-abi");
    fs::write(
        root.join("rust/src/lib.rs"),
        "pub fn library_without_rspyts() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("inspect")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("missing required rspyts discovery ABI v1 symbol"),
        "{stderr}"
    );
    assert!(
        stderr.contains("rspyts_discovery_v1_contract__fixture"),
        "{stderr}"
    );
    assert!(
        stderr.contains("pin the `rspyts` crate and CLI to the exact same version"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_discovery_symbol_is_rejected_with_abi_diagnostic() {
    let root = project("legacy-discovery-abi");
    fs::write(
        root.join("rust/src/lib.rs"),
        r#"
#[unsafe(export_name = "rspyts_contract")]
pub extern "C" fn legacy_contract() {}

#[unsafe(export_name = "rspyts_contract_free")]
pub extern "C" fn legacy_contract_free() {}
"#,
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("inspect")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("exports legacy unversioned rspyts discovery symbol `rspyts_contract`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("requires discovery ABI v1 symbol `rspyts_discovery_v1_contract__fixture`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("pin the `rspyts` crate and CLI to the exact same version"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_registry_is_a_normal_actionable_cli_error() {
    let root = project("malformed-registry");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(
        root.join("rust/Cargo.toml"),
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        ),
    )
    .unwrap();
    generate_lockfile(&root);
    fs::write(
        root.join("rust/src/lib.rs"),
        r#"
fn duplicate_type() -> rspyts::ir::TypeDef {
    rspyts::ir::TypeDef {
        owner: rspyts::ir::CargoPackageId::new(env!("CARGO_PKG_NAME")),
        id: "fixture::Item".to_owned(),
        name: "Item".to_owned(),
        docs: None,
        shape: rspyts::ir::TypeShape::Struct { fields: vec![] },
    }
}

rspyts::__private::inventory::submit! {
    rspyts::registry::TypeRegistration(duplicate_type)
}
rspyts::__private::inventory::submit! {
    rspyts::registry::TypeRegistration(duplicate_type)
}

rspyts::module!(native);
"#,
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("inspect")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        output.status.code().is_some(),
        "CLI was terminated by a signal instead of returning an error: {stderr}"
    );
    assert!(
        stderr.contains("failed rspyts contract discovery"),
        "{stderr}"
    );
    assert!(stderr.contains("invalid rspyts registry"), "{stderr}");
    assert!(stderr.contains("duplicate type identity"), "{stderr}");
    assert!(stderr.contains("fixture::Item"), "{stderr}");
    assert!(!stderr.contains("cannot unwind"), "{stderr}");
    assert!(!stderr.contains("SIGABRT"), "{stderr}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_has_no_arbitrary_staging_option() {
    let root = project("no-staging-option");
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .arg("--staging")
        .arg(root.join("rust"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--staging'"));
    assert!(root.join("rust/Cargo.toml").is_file());
    assert!(!root.join(".rspyts").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_report_names_the_fixed_generated_path_as_output() {
    let root = project("fixed-output-report");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(
        root.join("rust/Cargo.toml"),
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        ),
    )
    .unwrap();
    generate_lockfile(&root);
    fs::write(root.join("rust/src/lib.rs"), "rspyts::module!(native);\n").unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected_output = root.canonicalize().unwrap().join(".rspyts");
    assert_eq!(
        report["output"].as_str(),
        Some(expected_output.to_string_lossy().as_ref())
    );
    assert!(report.get("staging").is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn locked_check_validates_before_publishing_generated_output() {
    let root = project("locked-check-publication");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = |version| {
        format!(
            "[package]\nname = \"fixture\"\nversion = \"{version}\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        )
    };
    fs::write(root.join("rust/Cargo.toml"), manifest("0.1.0")).unwrap();
    generate_lockfile(&root);
    fs::write(root.join("rust/src/lib.rs"), "rspyts::module!(native);\n").unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    let config = root.join("rspyts.toml");

    for command in ["lock", "build"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
            .arg(command)
            .arg("--config")
            .arg(&config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let contract = fs::read(root.join(".rspyts/contract.json")).unwrap();
    fs::write(root.join(".rspyts/preserved"), "keep").unwrap();
    fs::write(root.join("rust/Cargo.toml"), manifest("0.1.1")).unwrap();
    generate_lockfile(&root);

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("check")
        .arg("--locked")
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("crate version `0.1.1` does not match locked version `0.1.0`"));
    assert_eq!(
        fs::read_to_string(root.join(".rspyts/preserved")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read(root.join(".rspyts/contract.json")).unwrap(),
        contract
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_check_and_clean_reject_authored_python_inside_fixed_output() {
    let root = project("authored-fixed-output");
    fs::create_dir_all(root.join(".rspyts/python-src/fixture")).unwrap();
    fs::write(
        root.join(".rspyts/python-src/fixture/authored.py"),
        "AUTHORED = True\n",
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\nsource = \".rspyts/python-src\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    for command in ["build", "check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
            .arg(command)
            .arg("--config")
            .arg(root.join("rspyts.toml"))
            .arg("--target")
            .arg("typescript")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("authored Python source"),
            "unexpected {command} error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let clean = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("clean")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(!clean.status.success());
    assert!(String::from_utf8_lossy(&clean.stderr).contains("authored Python source"));
    assert_eq!(
        fs::read_to_string(root.join(".rspyts/python-src/fixture/authored.py")).unwrap(),
        "AUTHORED = True\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_rejects_a_rust_crate_inside_fixed_output_without_mutating_it() {
    let root = project("authored-rust-fixed-output");
    fs::create_dir_all(root.join(".rspyts/rust/src")).unwrap();
    fs::rename(
        root.join("rust/Cargo.toml"),
        root.join(".rspyts/rust/Cargo.toml"),
    )
    .unwrap();
    fs::write(root.join(".rspyts/rust/src/lib.rs"), "authored Rust\n").unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \".rspyts/rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .arg("--target")
        .arg("typescript")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("authored Rust crate"));
    assert_eq!(
        fs::read_to_string(root.join(".rspyts/rust/src/lib.rs")).unwrap(),
        "authored Rust\n"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_rejects_a_dependency_lock_inside_fixed_output_without_mutating_it() {
    let root = project("authored-dependency-lock-fixed-output");
    fs::create_dir_all(root.join(".rspyts")).unwrap();
    fs::write(root.join(".rspyts/owner.lock"), "authored lock\n").unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n\n[dependencies.owner]\ncrate = \"owner\"\nlock = \".rspyts/owner.lock\"\ntypescript = \"owner\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .arg("--target")
        .arg("typescript")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("dependency `owner` lock"));
    assert_eq!(
        fs::read_to_string(root.join(".rspyts/owner.lock")).unwrap(),
        "authored lock\n"
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn build_check_and_clean_reject_a_symlinked_fixed_output_without_mutating_its_target() {
    use std::os::unix::fs::symlink;

    let root = project("symlinked-fixed-output");
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"static\"\n",
    )
    .unwrap();
    let target = root.with_extension("authored-output-target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("authored"), "keep\n").unwrap();
    symlink(&target, root.join(".rspyts")).unwrap();

    for command in ["build", "check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
            .arg(command)
            .arg("--config")
            .arg(root.join("rspyts.toml"))
            .arg("--target")
            .arg("typescript")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("may not be a symlink"));
    }
    let clean = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("clean")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(!clean.status.success());
    assert!(String::from_utf8_lossy(&clean.stderr).contains("may not be a symlink"));
    assert!(
        fs::symlink_metadata(root.join(".rspyts"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(target.join("authored")).unwrap(),
        "keep\n"
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(target).unwrap();
}

#[test]
fn wasm_native_exports_are_prefixed_away_from_wasm_bindgen_internals() {
    if !Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
        || !Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let root = project("prefixed-wasm-native-export");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(
        root.join("rust/Cargo.toml"),
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[features]\npython = []\nwasm = [\"rspyts/wasm\", \"dep:wasm-bindgen\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\nwasm-bindgen = {{ version = \"=0.2.126\", optional = true }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        ),
    )
    .unwrap();
    generate_lockfile(&root);
    fs::write(
        root.join("rust/src/lib.rs"),
        "#[rspyts::export(typescript)]\npub fn wasm() -> u32 { 7 }\n\npub struct Counter { value: u32 }\n\n#[rspyts::export(typescript)]\nimpl Counter {\n    #[rspyts(constructor)]\n    pub fn new(value: u32) -> Self { Self { value } }\n\n    #[rspyts(constructor)]\n    pub fn from_value(value: u32) -> Self { Self { value } }\n\n    pub fn read(&self) -> u32 { self.value }\n}\n\nrspyts::module!(native);\n",
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"wasm\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .arg("--target")
        .arg("typescript")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let native = fs::read_to_string(root.join(".rspyts/typescript/native.js")).unwrap();
    assert!(native.contains("export function __rspyts_export_wasm("));
    assert!(!native.contains("export function wasm("));
    assert!(native.contains("__rspyts_export_fromValue"));
    assert!(native.contains("__rspyts_export_read"));
    assert!(!native.contains("Symbol.dispose"));
    let native_declarations =
        fs::read_to_string(root.join(".rspyts/typescript/native.d.ts")).unwrap();
    assert!(!native_declarations.contains("Symbol.dispose"));
    let public = fs::read_to_string(root.join(".rspyts/typescript/index.js")).unwrap();
    assert!(public.contains("native.__rspyts_export_wasm("));
    assert!(public.contains("__rspyts_export_fromValue("));
    assert!(public.contains("__rspyts_export_read("));
    assert!(public.contains("free()"));
    assert!(!public.contains("Symbol.dispose"));
    let public_declarations =
        fs::read_to_string(root.join(".rspyts/typescript/index.d.ts")).unwrap();
    assert!(public_declarations.contains("free(): void;"));
    assert!(!public_declarations.contains("Symbol.dispose"));

    let import = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            "import { readFile } from 'node:fs/promises'; import { pathToFileURL } from 'node:url'; const entry = pathToFileURL(process.argv.at(-1)); const api = await import(entry.href); await api.default(await readFile(new URL('./native_bg.wasm', entry))); if (api.wasm() !== 7) throw new Error('function mismatch'); const direct = new api.Counter(11); if (direct.read() !== 11) throw new Error('method mismatch'); const factory = api.Counter.fromValue(13); if (factory.read() !== 13) throw new Error('factory mismatch'); direct.free(); factory.free();",
        ])
        .arg(root.join(".rspyts/typescript/index.js"))
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "generated package did not parse/import:\n{}{}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wrapper_hygiene_accepts_ordinary_parameter_names_in_generated_hosts() {
    if !Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
        || !Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let root = project("wrapper-hygiene-runtime");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(
        root.join("rust/Cargo.toml"),
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[features]\npython = [\"rspyts/python-extension\"]\nwasm = [\"rspyts/wasm\", \"dep:wasm-bindgen\"]\n\n[dependencies]\nrspyts = {{ path = {:?} }}\nwasm-bindgen = {{ version = \"=0.2.126\", optional = true }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        ),
    )
    .unwrap();
    generate_lockfile(&root);
    fs::write(
        root.join("rust/src/lib.rs"),
        r#"
#[rspyts::export]
pub fn ordinary_names(py: u32, ty: u32, value: u32, error: u32, types: u32, wire: u32) -> u32 {
    py + ty + value + error + types + wire
}

mod exported_constants {
    #[allow(non_upper_case_globals)]
    #[rspyts::export]
    pub const ty: u32 = 11;

    #[allow(non_upper_case_globals)]
    #[rspyts::export]
    pub const types: u32 = 12;

    #[allow(non_upper_case_globals)]
    #[rspyts::export]
    pub const value: u32 = 13;

    #[allow(non_upper_case_globals)]
    #[rspyts::export]
    pub const error: u32 = 14;

    #[allow(non_upper_case_globals)]
    #[rspyts::export]
    pub const wire: u32 = 15;
}

pub struct Counter(u32);

#[rspyts::export]
impl Counter {
    #[rspyts(constructor)]
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn inner(&self, inner: u32, error: u32, types: u32, wire: u32) -> u32 {
        self.0 + inner + error + types + wire
    }
}

rspyts::module!(native);
"#,
    )
    .unwrap();
    fs::write(
        root.join("rspyts.toml"),
        "[crate]\npath = \"rust\"\n\n[python]\npackage = \"fixture\"\n\n[typescript]\npackage = \"fixture\"\nmode = \"wasm\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rspyts"))
        .arg("build")
        .arg("--config")
        .arg(root.join("rspyts.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let python = root.join(".rspyts/python/fixture");
    assert!(
        fs::read_to_string(python.join("functions.py"))
            .unwrap()
            .contains("def ordinary_names(")
    );
    assert!(
        fs::read_to_string(python.join("resources.py"))
            .unwrap()
            .contains("class Counter:")
    );
    assert!(
        fs::read_to_string(python.join("constants.py"))
            .unwrap()
            .contains(
                "wire: __rspyts_Annotated__[__rspyts_builtins__.int, __rspyts_Field__(ge=0, le=4294967295)] = 15"
            )
    );
    assert!(!python.join("native.so").exists());
    assert!(!python.join("native.pyd").exists());

    let node_output = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            "import { readFile } from 'node:fs/promises'; import { pathToFileURL } from 'node:url'; const entry = pathToFileURL(process.argv.at(-1)); const api = await import(entry.href); await api.default(await readFile(new URL('./native_bg.wasm', entry))); if (api.ordinaryNames(1, 2, 3, 4, 5, 6) !== 21) throw new Error('function mismatch'); if ([api.ty, api.types, api.value, api.error, api.wire].join(',') !== '11,12,13,14,15') throw new Error('constant mismatch'); const counter = new api.Counter(10); if (counter.inner(1, 2, 3, 4) !== 20) throw new Error('method mismatch'); counter.free();",
        ])
        .arg(root.join(".rspyts/typescript/index.js"))
        .output()
        .unwrap();
    assert!(
        node_output.status.success(),
        "generated TypeScript package failed at runtime:\n{}{}",
        String::from_utf8_lossy(&node_output.stdout),
        String::from_utf8_lossy(&node_output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wrapper_hygiene_rejects_the_generated_parameter_namespace() {
    let root = project("wrapper-hygiene-reserved-prefix");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(
        root.join("rust/Cargo.toml"),
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nrspyts = {{ path = {:?} }}\n\n[patch.crates-io]\nrspyts-macros = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("rspyts").to_string_lossy(),
            crates.join("rspyts-macros").to_string_lossy()
        ),
    )
    .unwrap();
    generate_lockfile(&root);
    fs::write(
        root.join("rust/src/lib.rs"),
        "#[rspyts::export]\npub fn invalid_type(__rspyts_type: u32) -> u32 { __rspyts_type }\n\n#[rspyts::export]\npub fn invalid_types(__rspyts_types: u32) -> u32 { __rspyts_types }\n",
    )
    .unwrap();

    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg("check")
        .arg("--manifest-path")
        .arg(root.join("rust/Cargo.toml"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("parameter `__rspyts_type`"), "{stderr}");
    assert!(stderr.contains("parameter `__rspyts_types`"), "{stderr}");
    assert!(
        stderr.contains("reserved `__rspyts_` prefix")
            && stderr.contains("generated rspyts wrapper bindings"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}
