use std::path::{Path, PathBuf};

fn markdown_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read documentation directory") {
        let path = entry.expect("read documentation entry").path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

#[test]
fn published_documentation_uses_the_current_api() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !workspace.join("docs").is_dir() {
        return;
    }
    let mut files = vec![
        workspace.join("README.md"),
        workspace.join("crates/rspyts-macros/src/lib.rs"),
    ];
    markdown_files(&workspace.join("docs"), &mut files);
    markdown_files(&workspace.join("crates"), &mut files);

    let retired = [
        "ABI version 2",
        "ABI-2 request",
        "registerError",
        "--profile release",
        "`I64` and `U64`",
        "manifest diffs",
    ];
    for path in files {
        let content = std::fs::read_to_string(&path).expect("read documentation file");
        for spelling in retired {
            assert!(
                !content.contains(spelling),
                "{} still contains retired spelling {spelling:?}",
                path.display(),
            );
        }
    }

    let codegen = std::fs::read_to_string(workspace.join("docs/design/codegen.md"))
        .expect("read code generation documentation");
    assert!(codegen.contains("codecs.py"));
    assert!(codegen.contains("codecs.ts"));
}
