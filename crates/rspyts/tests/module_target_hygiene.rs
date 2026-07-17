use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(rspyts: &Path) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rspyts-python-only-module-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).expect("create Python-only module fixture");
        let rspyts = format!("{:?}", rspyts.to_string_lossy());
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[workspace]\n\
                 \n\
                 [package]\n\
                 name = \"rspyts-python-only-module-fixture\"\n\
                 version = \"0.0.0\"\n\
                 edition = \"2024\"\n\
                 publish = false\n\
                 \n\
                 [lib]\n\
                 crate-type = [\"cdylib\"]\n\
                 \n\
                 [features]\n\
                 default = []\n\
                 python = [\"rspyts/python-extension\"]\n\
                 \n\
                 [dependencies]\n\
                 rspyts = {{ path = {rspyts}, default-features = false }}\n"
            ),
        )
        .expect("write Python-only module fixture manifest");
        fs::write(
            root.join("src/lib.rs"),
            "#![deny(warnings)]\n\nrspyts::module!(native, python);\n",
        )
        .expect("write Python-only module fixture source");
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn python_only_module_does_not_emit_unknown_wasm_feature_cfg() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("canonical rspyts crate directory");
    let fixture = Fixture::new(&crate_dir);
    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--manifest-path",
            fixture.root.join("Cargo.toml").to_str().unwrap(),
            "--features",
            "python",
        ])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", fixture.root.join("target"))
        .env("RUSTFLAGS", "-Dwarnings")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("run the Python-only module fixture check");

    assert!(
        output.status.success(),
        "Python-only module fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
