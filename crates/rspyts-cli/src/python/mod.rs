//! Python package emission and source-project validation.
//!
//! Generated modules are rendered into a temporary tree. The package manifest
//! and root `__init__.py` remain user-owned and are validated but never
//! rewritten during a build.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rspyts::ir::{
    BufferElement, DefinitionId, FieldDef, FunctionDef, Manifest, Namespace, ParamDef, ResourceDef,
    ScalarValue, TypeDef, TypeRef, TypeShape,
};
use serde_json::Value;

use crate::contract::{
    NamespaceItems, collect_buffers, collect_buffers_resolved, definition_key, error_definition,
    named_identities, namespace_refs, namespaces, reference_contains, tagged_variant_name,
    type_definition, type_namespace, type_refs, uses_buffer,
};
use crate::output::write;
use crate::project::Project;

mod api;
mod models;
mod render;

use api::*;
use models::*;
/// Python syntax and reference-rendering primitives shared by emitters.
pub(crate) use render::*;

/// Shared contract and namespace state for one generated Python module.
pub(super) struct PythonContext<'a> {
    manifest: &'a Manifest,
    package: &'a str,
    namespace: &'a Namespace,
}

/// Emit the complete set of rspyts-owned Python files into a staging tree.
pub(super) fn emit(
    project: &Project,
    manifest: &Manifest,
    native: &Path,
    root: &Path,
) -> Result<()> {
    let python_root = root.join("src-py");
    let package = python_root.join(project.python_package.replace('.', "/"));
    fs::create_dir_all(&package)?;

    let native_package = package.join(&manifest.module_name);
    fs::create_dir_all(&native_package)?;
    let extension = if cfg!(windows) { "pyd" } else { "abi3.so" };
    fs::copy(
        native,
        native_package.join(format!("{}.{}", manifest.module_name, extension)),
    )
    .with_context(|| format!("failed to copy Python extension {}", native.display()))?;
    write(
        &native_package.join("__init__.py"),
        &generated_python(&format!(
            "from . import {0} as {0}\n\n__all__ = [{0:?}]\n",
            manifest.module_name
        )),
    )?;
    write(
        &package.join("runtime.py"),
        &generated_python_module(
            &[
                "native".to_owned(),
                "native_error".to_owned(),
                "prepare_host".to_owned(),
                "restore_host".to_owned(),
            ],
            &python_runtime(manifest)?,
        ),
    )?;
    write(&package.join("py.typed"), GENERATED_HEADER)?;

    let namespace_map = namespaces(manifest);
    for namespace in namespace_map.keys() {
        let segments = namespace.python_segments();
        let mut namespace_package = package.clone();
        for segment in &segments {
            namespace_package.push(segment);
            fs::create_dir_all(&namespace_package)?;
        }
    }
    for (namespace, items) in &namespace_map {
        let namespace_package = namespace
            .python_segments()
            .iter()
            .fold(package.clone(), |path, segment| path.join(segment));
        let context = PythonContext {
            manifest,
            package: &project.python_package,
            namespace,
        };
        let (model_names, api_names) = python_export_names(items);
        write(
            &namespace_package.join("models.py"),
            &generated_python_module(&model_names, &python_models(items, &context)?),
        )?;
        write(
            &namespace_package.join("api.py"),
            &generated_python_module(&api_names, &python_api(items, &context)?),
        )?;
        if *namespace != Namespace::root() {
            write(
                &namespace_package.join("__init__.py"),
                &generated_python(&python_init()),
            )?;
        }
    }
    Ok(())
}

/// Validate the user-owned Python project before generated files are published.
pub(super) fn validate_project(project: &Project, manifest: &Manifest) -> Result<()> {
    validate_project_dependencies(
        &project.python_source().join("pyproject.toml"),
        &project.package_version,
        uses_buffer(manifest),
    )
}

/// Validate package version and generated runtime dependencies.
fn validate_project_dependencies(path: &Path, version: &str, needs_numpy: bool) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read user-owned {}", path.display()))?;
    let value = source
        .parse::<toml::Value>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let project = value
        .get("project")
        .context("Python project must define `[project]`")?;
    let declared_version = project
        .get("version")
        .and_then(toml::Value::as_str)
        .context("Python project must define `[project].version`")?;
    if declared_version != version {
        bail!(
            "{} declares version `{declared_version}`, but Cargo.toml declares `{version}`",
            path.display()
        );
    }
    let dependencies = project
        .get("dependencies")
        .and_then(toml::Value::as_array)
        .context("Python project must define `[project].dependencies`")?;
    for required in ["pydantic"]
        .into_iter()
        .chain(needs_numpy.then_some("numpy"))
    {
        let present = dependencies
            .iter()
            .filter_map(toml::Value::as_str)
            .any(|item| {
                item.split(|character: char| {
                    !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '.')
                })
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(required))
            });
        if !present {
            bail!(
                "{} must declare the generated runtime dependency `{required}`",
                path.display()
            );
        }
    }
    Ok(())
}

const GENERATED_HEADER: &str = "# =============================================================================\n# AUTO-GENERATED BY rspyts - DO NOT EDIT.\n#\n# This file is overwritten by `rspyts build`.\n# =============================================================================\n\n";

/// Prefix generated Python source with the ownership warning.
fn generated_python(source: &str) -> String {
    format!("{GENERATED_HEADER}{source}")
}

/// Prefix a module and append its sorted, explicit export list.
fn generated_python_module(exports: &[String], source: &str) -> String {
    let mut exports = exports.to_vec();
    exports.sort();
    let mut generated = String::from(GENERATED_HEADER);
    generated.push_str(source);
    if !generated.ends_with('\n') {
        generated.push('\n');
    }
    generated.push('\n');
    generated.push_str("__all__ = [\n");
    for name in &exports {
        writeln!(generated, "    {},", py_string(name)).expect("writing to String cannot fail");
    }
    generated.push_str("]\n");
    generated
}

/// Render the wildcard re-export entrypoint for a generated subpackage.
fn python_init() -> String {
    "from .models import *\nfrom .models import __all__ as __models__\n\nfrom .api import *\nfrom .api import __all__ as __api__\n\n__all__ = [*__models__, *__api__]\n".to_owned()
}

/// Compute the model and API symbols exported by one namespace.
fn python_export_names(items: &NamespaceItems<'_>) -> (Vec<String>, Vec<String>) {
    let mut model_names = python_model_names(items);
    let mut api_names = items
        .errors
        .iter()
        .map(|item| item.name.clone())
        .chain(items.functions.iter().map(|item| item.rust_name.clone()))
        .chain(items.resources.iter().map(|item| item.name.clone()))
        .chain(items.constants.iter().map(|item| item.host_name.clone()))
        .collect::<Vec<_>>();
    model_names.sort();
    api_names.sort();
    (model_names, api_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_modules_have_a_warning_and_sorted_explicit_exports() {
        let generated = generated_python_module(
            &["Zulu".to_owned(), "Alpha".to_owned()],
            "from __future__ import annotations\n",
        );

        assert!(generated.starts_with(GENERATED_HEADER));
        assert!(generated.ends_with("__all__ = [\n    \"Alpha\",\n    \"Zulu\",\n]\n"));
    }

    #[test]
    fn generated_empty_modules_still_declare_exports() {
        let generated = generated_python_module(&[], "from __future__ import annotations\n");

        assert!(generated.starts_with(GENERATED_HEADER));
        assert!(generated.ends_with("__all__ = [\n]\n"));
    }
}
