use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, FieldDef, FunctionDef, Manifest, Namespace, ParamDef, ResourceDef, ScalarValue,
    TypeDef, TypeRef, TypeShape,
};
use serde_json::Value;

use crate::contract::{
    NamespaceItems, collect_buffers, definition_key, error_definition, named_identities,
    namespace_refs, namespaces, reference_contains, tagged_variant_name, type_definition,
    type_namespace, type_refs, uses_buffer,
};
use crate::output::write;
use crate::project::Project;

mod api;
mod models;
mod render;

use api::*;
use models::*;
pub(crate) use render::*;

pub(super) struct PythonContext<'a> {
    manifest: &'a Manifest,
    package: &'a str,
    namespace: &'a Namespace,
}

pub(super) fn emit(
    project: &Project,
    manifest: &Manifest,
    native: &Path,
    root: &Path,
) -> Result<()> {
    let python_root = root.join("python");
    let package = python_root.join(project.python_package.replace('.', "/"));
    fs::create_dir_all(&package)?;
    let mut parent = python_root.clone();
    for segment in project
        .python_package
        .split('.')
        .collect::<Vec<_>>()
        .iter()
        .take(project.python_package.split('.').count().saturating_sub(1))
    {
        parent.push(segment);
        write(&parent.join("__init__.py"), "")?;
    }

    let extension = if cfg!(windows) { "pyd" } else { "so" };
    fs::copy(
        native,
        package.join(format!("{}.{}", manifest.module_name, extension)),
    )
    .with_context(|| format!("failed to copy Python extension {}", native.display()))?;
    write(&package.join("runtime.py"), &python_runtime(manifest)?)?;
    write(&package.join("py.typed"), "")?;

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
        let model_names = python_model_names(items);
        let has_api = !items.errors.is_empty()
            || !items.functions.is_empty()
            || !items.resources.is_empty()
            || !items.constants.is_empty();
        if !model_names.is_empty() {
            write(
                &namespace_package.join("models.py"),
                &python_models(items, &context)?,
            )?;
        }
        if has_api {
            write(
                &namespace_package.join("api.py"),
                &python_api(items, &context)?,
            )?;
        }
        write(&namespace_package.join("__init__.py"), &python_init(items))?;
        write(
            &namespace_package.join("__init__.pyi"),
            &python_init_stub(items),
        )?;
    }
    for namespace in namespace_map.keys() {
        let mut namespace_package = package.clone();
        for segment in namespace.python_segments() {
            namespace_package.push(segment);
            let init = namespace_package.join("__init__.py");
            if !init.exists() {
                write(&init, "")?;
            }
            let init_stub = namespace_package.join("__init__.pyi");
            if !init_stub.exists() {
                write(&init_stub, "")?;
            }
        }
    }

    let pyproject = python_project_file(
        &project.python_package.replace('.', "-"),
        &manifest.package_version,
        uses_buffer(manifest),
    );
    write(&python_root.join("pyproject.toml"), &pyproject)?;
    write(
        &python_root.join("setup.py"),
        "from setuptools import Distribution, setup\n\n\nclass BinaryDistribution(Distribution):\n    def has_ext_modules(self) -> bool:\n        return True\n\n\nsetup(distclass=BinaryDistribution)\n",
    )?;
    write(
        &python_root.join("setup.cfg"),
        "[bdist_wheel]\npy_limited_api = cp311\n",
    )
}

fn python_project_file(distribution: &str, version: &str, needs_numpy: bool) -> String {
    let dependencies = if needs_numpy {
        "dependencies = [\"pydantic>=2,<3\", \"numpy>=2,<3\"]"
    } else {
        "dependencies = [\"pydantic>=2,<3\"]"
    };
    format!(
        "[build-system]\nrequires = [\"setuptools>=77\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = {}\nversion = {}\nrequires-python = \">=3.11\"\n{}\n\n[tool.setuptools.packages.find]\nwhere = [\".\"]\n\n[tool.setuptools.package-data]\n\"*\" = [\"*.pyi\", \"*.so\", \"*.pyd\", \"py.typed\"]\n",
        py_string(distribution),
        py_string(version),
        dependencies,
    )
}

fn python_init(items: &NamespaceItems<'_>) -> String {
    let (model_names, api_names) = python_export_names(items);
    let mut source = String::from("\"\"\"Generated from the Rust application API.\"\"\"\n\n");
    if model_names.is_empty() && api_names.is_empty() {
        source.push_str("__all__: list[str] = []\n");
        return source;
    }
    source.push_str("__all__ = [\n");
    for name in model_names.iter().chain(&api_names) {
        writeln!(source, "    {},", py_string(name)).unwrap();
    }
    source.push_str("]\n\n");
    source.push_str(
        "def __getattr__(\n    name: str,\n    _exports: dict[str, tuple[str, str]] = {\n",
    );
    for name in &model_names {
        writeln!(
            source,
            "        {}: (\".models\", {}),",
            py_string(name),
            py_string(name)
        )
        .unwrap();
    }
    for name in &api_names {
        writeln!(
            source,
            "        {}: (\".api\", {}),",
            py_string(name),
            py_string(name)
        )
        .unwrap();
    }
    source.push_str(
        "    },\n    _package: str = __name__,\n) -> object:\n    from builtins import AttributeError as builtin_attribute_error\n    from builtins import KeyError as builtin_key_error\n    from builtins import getattr as builtin_getattr\n    from importlib import import_module\n    from sys import modules\n\n    try:\n        module_name, member_name = _exports[name]\n    except builtin_key_error:\n        raise builtin_attribute_error(name) from None\n    value = builtin_getattr(import_module(module_name, _package), member_name)\n    modules[_package].__dict__[name] = value\n    return value\n\n\ndef __dir__(\n    _exports: tuple[str, ...] = tuple(__all__),\n    _package: str = __name__,\n) -> list[str]:\n    from builtins import set as builtin_set\n    from builtins import sorted as builtin_sorted\n    from sys import modules\n\n    return builtin_sorted(builtin_set(modules[_package].__dict__) | builtin_set(_exports))\n",
    );
    source
}

fn python_init_stub(items: &NamespaceItems<'_>) -> String {
    let (model_names, api_names) = python_export_names(items);
    let mut source = String::from("\"\"\"Generated from the Rust application API.\"\"\"\n\n");
    if model_names.is_empty() && api_names.is_empty() {
        source.push_str("__all__: list[str]\n");
        return source;
    }
    if !api_names.is_empty() {
        source.push_str("from .api import (\n");
        for name in &api_names {
            writeln!(source, "    {name} as {name},").unwrap();
        }
        source.push_str(")\n");
    }
    if !model_names.is_empty() {
        source.push_str("from .models import (\n");
        for name in &model_names {
            writeln!(source, "    {name} as {name},").unwrap();
        }
        source.push_str(")\n");
    }
    source.push_str("\n__all__ = [\n");
    for name in model_names.iter().chain(&api_names) {
        writeln!(source, "    {},", py_string(name)).unwrap();
    }
    source.push_str("]\n");
    source
}

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
