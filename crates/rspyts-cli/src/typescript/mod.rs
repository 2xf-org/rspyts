use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, FieldConstraints, FieldDef, FunctionDef, Manifest, Namespace, ParamDef,
    ResourceDef, TypeDef, TypeRef, TypeShape,
};
use serde_json::{Value, json};
use wasm_bindgen_cli_support::Bindgen;

use crate::contract::{
    NamespaceItems, definition_key, error_definition, named_identities, namespace_refs, namespaces,
    reference_contains, tagged_variant_name, type_definition, type_namespace,
};
use crate::output::{write, write_json};
use crate::project::{Project, is_identifier};

mod api;
mod declarations;
mod render;

use api::*;
use declarations::*;
pub(crate) use render::*;

pub(super) struct TypeScriptContext<'a> {
    manifest: &'a Manifest,
    package: &'a str,
    namespace: &'a Namespace,
}

pub(super) fn emit(project: &Project, manifest: &Manifest, wasm: &Path, root: &Path) -> Result<()> {
    let package = root.join("typescript");
    fs::create_dir_all(&package)?;
    let mut bindgen = Bindgen::new();
    bindgen
        .input_path(wasm)
        .web(true)?
        .typescript(false)
        .omit_default_module_path(false)
        .out_name("native")
        .generate(&package)
        .context("failed to generate TypeScript WebAssembly bindings")?;
    write(&package.join("runtime.js"), &typescript_runtime(manifest)?)?;
    write(&package.join("values.js"), TYPESCRIPT_VALUES)?;
    let namespace_map = namespaces(manifest);
    for (namespace, items) in &namespace_map {
        let namespace_package = namespace
            .typescript_segments()
            .iter()
            .fold(package.clone(), |path, segment| path.join(segment));
        fs::create_dir_all(&namespace_package)?;
        let context = TypeScriptContext {
            manifest,
            package: &project.typescript_package,
            namespace,
        };
        write(
            &namespace_package.join("index.d.ts"),
            &typescript_declarations(items, &context)?,
        )?;
        write(
            &namespace_package.join("index.js"),
            &typescript_api(items, &context)?,
        )?;
    }
    let package_json = package_manifest(
        &project.typescript_package,
        &manifest.package_version,
        namespace_map.keys(),
    );
    write_json(&package.join("package.json"), &package_json)
}

fn package_manifest<'a>(
    package_name: &str,
    package_version: &str,
    namespaces: impl Iterator<Item = &'a Namespace>,
) -> Value {
    let mut exports = serde_json::Map::new();
    for namespace in namespaces {
        let segments = namespace.typescript_segments();
        let subpath = if segments.is_empty() {
            ".".to_owned()
        } else {
            format!("./{}", segments.join("/"))
        };
        let file = if segments.is_empty() {
            "./index".to_owned()
        } else {
            format!("./{}/index", segments.join("/"))
        };
        exports.insert(
            subpath,
            json!({
                "types": format!("{file}.d.ts"),
                "import": format!("{file}.js")
            }),
        );
    }
    json!({
        "name": package_name,
        "version": package_version,
        "type": "module",
        "sideEffects": true,
        "types": "./index.d.ts",
        "exports": exports,
        "files": ["**/*.js", "**/*.d.ts", "native_bg.wasm"]
    })
}
