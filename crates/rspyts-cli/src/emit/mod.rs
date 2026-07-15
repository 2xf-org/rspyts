//! Emitter orchestration: render everything in memory, then either
//! write (`generate`) or diff (`check`) against the filesystem.
//!
//! Output directories are wholly owned: `generate` deletes files it no
//! longer emits, but only `.py`/`.ts` files whose first line carries the
//! rspyts header — user files that stray into an out dir are never
//! touched.

pub mod python;
pub mod schema;
pub mod typescript;
pub mod util;

#[cfg(test)]
pub mod test_manifest;

use crate::config::Config;
use anyhow::{Context, Result};
use rspyts_core::ir::Manifest;
use std::path::{Path, PathBuf};

/// One file the emitters want on disk.
pub struct OutFile {
    pub path: PathBuf,
    pub content: String,
}

/// A directory the generator owns, with the file names it emits there.
pub struct OwnedDir {
    pub dir: PathBuf,
    pub keep: Vec<&'static str>,
}

/// Everything a `generate`/`check` run would put on disk.
pub struct Plan {
    pub files: Vec<OutFile>,
    pub owned: Vec<OwnedDir>,
}

/// Render every configured emitter into a [`Plan`]. Pure: no filesystem.
pub fn plan(cfg: &Config, manifest: &Manifest, hash: &str) -> Plan {
    let mut files = Vec::new();
    let mut owned = Vec::new();
    let mut add = |dir: &Path, emitted: Vec<(&'static str, String)>| {
        owned.push(OwnedDir {
            dir: dir.to_path_buf(),
            keep: emitted.iter().map(|(n, _)| *n).collect(),
        });
        files.extend(emitted.into_iter().map(|(name, content)| OutFile {
            path: dir.join(name),
            content,
        }));
    };
    if let Some(py) = &cfg.python {
        let library_search = ["lib".to_string()];
        add(
            &py.out,
            python::emit(manifest, hash, &library_search, &py.imports),
        );
    }
    if let Some(ts) = &cfg.typescript {
        add(&ts.out, typescript::emit(manifest, hash, &ts.imports));
    }
    if let Some(sc) = &cfg.schema {
        add(&sc.out, schema::emit(manifest, hash));
    }
    Plan { files, owned }
}

/// `rspyts generate`: write changed files, delete no-longer-emitted
/// ones, print one line per file. Untouched files keep their mtime.
pub fn write(plan: &Plan) -> Result<()> {
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create `{}`", parent.display()))?;
        }
        match std::fs::read_to_string(&file.path) {
            Ok(existing) if existing == file.content => {
                println!("unchanged  {}", file.path.display());
            }
            _ => {
                std::fs::write(&file.path, &file.content)
                    .with_context(|| format!("cannot write `{}`", file.path.display()))?;
                println!("wrote      {}", file.path.display());
            }
        }
    }
    for dir in &plan.owned {
        for stray in stray_generated_files(dir)? {
            std::fs::remove_file(&stray)
                .with_context(|| format!("cannot delete `{}`", stray.display()))?;
            println!("deleted    {}", stray.display());
        }
    }
    Ok(())
}

/// `rspyts check`: report stale/missing/unexpected files with unified
/// diffs on stderr. Returns `true` when anything drifted.
pub fn check(plan: &Plan) -> Result<bool> {
    let mut drift = false;
    for file in &plan.files {
        let existing = std::fs::read_to_string(&file.path).ok();
        match existing {
            Some(ref on_disk) if on_disk == &file.content => {}
            Some(on_disk) => {
                drift = true;
                eprintln!("stale: {}", file.path.display());
                print_diff(&on_disk, &file.content, &file.path);
            }
            None => {
                drift = true;
                eprintln!("missing: {}", file.path.display());
                print_diff("", &file.content, &file.path);
            }
        }
    }
    for dir in &plan.owned {
        for stray in stray_generated_files(dir)? {
            drift = true;
            eprintln!("unexpected generated file: {}", stray.display());
        }
    }
    Ok(drift)
}

fn print_diff(on_disk: &str, generated: &str, path: &Path) {
    let diff = similar::TextDiff::from_lines(on_disk, generated);
    eprint!(
        "{}",
        diff.unified_diff()
            .context_radius(3)
            .header(&format!("{} (on disk)", path.display()), "generated")
    );
}

/// Generated `.py`/`.ts` files in `dir` that this plan no longer emits.
/// Conservative: only files whose first line carries the rspyts header
/// qualify — anything else in the directory is left alone.
fn stray_generated_files(owned: &OwnedDir) -> Result<Vec<PathBuf>> {
    let mut strays = Vec::new();
    let entries = match std::fs::read_dir(&owned.dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(strays), // out dir does not exist yet
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot list `{}`", owned.dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("py" | "ts")) {
            continue;
        }
        let name = entry.file_name();
        if owned
            .keep
            .iter()
            .any(|k| std::ffi::OsStr::new(k) == name.as_os_str())
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue; // non-UTF-8: certainly not ours
        };
        if content
            .lines()
            .next()
            .is_some_and(|l| l.contains(util::GENERATED_MARKER))
        {
            strays.push(path);
        }
    }
    strays.sort();
    Ok(strays)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PythonConfig, SchemaConfig, TypescriptConfig};
    use test_manifest::{manifest, manifest_hash};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspyts-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config_for(root: &Path) -> Config {
        Config {
            crate_dir: root.join("crate"),
            contract: Default::default(),
            python: Some(PythonConfig {
                out: root.join("py"),
                imports: Default::default(),
            }),
            typescript: Some(TypescriptConfig {
                out: root.join("ts"),
                imports: Default::default(),
            }),
            schema: Some(SchemaConfig {
                out: root.join("schema"),
            }),
        }
    }

    #[test]
    fn plan_covers_all_configured_out_dirs() {
        let m = manifest();
        let hash = manifest_hash(&m);
        let root = PathBuf::from("/virtual");
        let plan = plan(&config_for(&root), &m, &hash);
        assert_eq!(plan.files.len(), 8 + 6 + 1);
        assert_eq!(plan.owned.len(), 3);
        assert!(
            plan.files
                .iter()
                .any(|f| f.path == root.join("py/__init__.py"))
        );
        assert!(
            plan.files
                .iter()
                .any(|f| f.path == root.join("py/codecs.py"))
        );
        assert!(
            plan.files
                .iter()
                .any(|f| f.path == root.join("ts/index.ts"))
        );
        assert!(
            plan.files
                .iter()
                .any(|f| f.path == root.join("ts/codecs.ts"))
        );
        assert!(
            plan.files
                .iter()
                .any(|f| f.path == root.join("schema/schema.json"))
        );
        let python_owned = plan
            .owned
            .iter()
            .find(|owned| owned.dir == root.join("py"))
            .expect("Python output directory is owned");
        assert!(python_owned.keep.contains(&"codecs.py"));
        let typescript_owned = plan
            .owned
            .iter()
            .find(|owned| owned.dir == root.join("ts"))
            .expect("TypeScript output directory is owned");
        assert!(typescript_owned.keep.contains(&"codecs.ts"));
    }

    #[test]
    fn write_then_check_round_trips_and_deletes_strays() {
        let m = manifest();
        let hash = manifest_hash(&m);
        let root = temp_dir("write");
        let cfg = config_for(&root);
        let p = plan(&cfg, &m, &hash);

        write(&p).unwrap();
        assert!(!check(&p).unwrap(), "freshly written tree must be clean");

        // A stale file and a stray generated file are both detected...
        let target = root.join("py/models.py");
        std::fs::write(&target, "# tampered\n").unwrap();
        let stray = root.join("py/old_module.py");
        std::fs::write(
            &stray,
            format!("# {} v0. DO NOT EDIT.\n", util::GENERATED_MARKER),
        )
        .unwrap();
        let user_file = root.join("py/handwritten.py");
        std::fs::write(&user_file, "print('mine')\n").unwrap();
        assert!(check(&p).unwrap());

        // ...and generate repairs the tree: rewrites the stale file,
        // deletes the stray, leaves the user's file alone.
        write(&p).unwrap();
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .starts_with("# Code generated")
        );
        assert!(!stray.exists());
        assert!(user_file.exists());
        assert!(!check(&p).unwrap());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_reports_missing_files_without_touching_disk() {
        let m = manifest();
        let hash = manifest_hash(&m);
        let root = temp_dir("check");
        let p = plan(&config_for(&root), &m, &hash);
        assert!(check(&p).unwrap());
        assert!(!root.join("py").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
