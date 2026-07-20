use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, FieldDef, FunctionDef, Manifest, ParamDef, ResourceDef, ScalarValue, TypeDef,
    TypeRef, TypeShape,
};
use serde_json::Value;

use crate::contract::{
    buffer_elements, collect_buffers, error_name, tagged_variant_name, type_name, uses_buffer,
};
use crate::output::write;
use crate::project::Project;

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
    write(&package.join("models.py"), &python_models(manifest)?)?;
    write(&package.join("runtime.py"), &python_runtime(manifest)?)?;
    write(&package.join("api.py"), &python_api(manifest)?)?;
    write(&package.join("__init__.py"), &python_init(manifest))?;
    write(&package.join("py.typed"), "")?;

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
        "[build-system]\nrequires = [\"setuptools>=77\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = {}\nversion = {}\nrequires-python = \">=3.11\"\n{}\n\n[tool.setuptools.packages.find]\nwhere = [\".\"]\n\n[tool.setuptools.package-data]\n\"*\" = [\"*.so\", \"*.pyd\", \"py.typed\"]\n",
        py_string(distribution),
        py_string(version),
        dependencies,
    )
}

fn python_models(manifest: &Manifest) -> Result<String> {
    let imports = model_imports(manifest);
    let buffers = buffer_elements(manifest);
    let mut source = String::from("from __future__ import annotations\n");
    let mut standard_imports = Vec::new();
    if imports.datetime {
        standard_imports.push("from datetime import datetime".to_owned());
    }
    if imports.string_enum {
        standard_imports.push("from enum import StrEnum".to_owned());
    }
    if !imports.typing.is_empty() {
        standard_imports.push(format!(
            "from typing import {}",
            imports.typing.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !standard_imports.is_empty() {
        source.push('\n');
        source.push_str(&standard_imports.join("\n"));
        source.push('\n');
    }
    let mut package_imports = Vec::new();
    if !buffers.is_empty() {
        package_imports.extend([
            "import numpy as np".to_owned(),
            "from numpy.typing import NDArray".to_owned(),
        ]);
    }
    if !imports.pydantic.is_empty() {
        package_imports.push(format!(
            "from pydantic import {}",
            imports.pydantic.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !buffers.is_empty() {
        package_imports.extend([
            "from pydantic.functional_serializers import PlainSerializer".to_owned(),
            "from pydantic.functional_validators import BeforeValidator".to_owned(),
        ]);
    }
    if !package_imports.is_empty() {
        source.push('\n');
        source.push_str(&package_imports.join("\n"));
        source.push('\n');
    }
    for element in buffers {
        let name = buffer_name(element);
        let scalar = python_numpy_scalar(element);
        begin_python_alias(&mut source);
        writeln!(
            source,
            "{name}: TypeAlias = Annotated[\n    NDArray[np.{scalar}],\n    BeforeValidator(lambda value: np.asarray(value, dtype=np.{scalar})),\n    PlainSerializer(lambda value: value.tolist(), return_type=list),\n]"
        )?;
    }

    for definition in &manifest.types {
        emit_python_type(&mut source, definition, manifest)?;
    }
    let mut rebuilds = Vec::new();
    for definition in &manifest.types {
        match definition.shape {
            TypeShape::Struct { .. } | TypeShape::Alias { .. } => {
                rebuilds.push(definition.name.clone());
            }
            TypeShape::TaggedEnum { ref variants, .. } => {
                for variant in variants {
                    rebuilds.push(tagged_variant_name(&definition.name, &variant.rust_name));
                }
            }
            TypeShape::StringEnum { .. } => {}
        }
    }
    if !rebuilds.is_empty() {
        begin_python_top_level(&mut source);
        for name in rebuilds {
            writeln!(source, "{name}.model_rebuild()")?;
        }
    }
    Ok(source)
}

#[derive(Default)]
struct ModelImports {
    datetime: bool,
    string_enum: bool,
    typing: BTreeSet<&'static str>,
    pydantic: BTreeSet<&'static str>,
}

fn model_imports(manifest: &Manifest) -> ModelImports {
    let mut imports = ModelImports::default();
    if uses_buffer(manifest) {
        imports.typing.extend(["Annotated", "TypeAlias"]);
    }
    for definition in &manifest.types {
        match &definition.shape {
            TypeShape::Struct { fields } => {
                imports.pydantic.extend(["BaseModel", "ConfigDict"]);
                if !fields.is_empty() {
                    imports.pydantic.insert("Field");
                }
                collect_field_imports(fields, &mut imports);
            }
            TypeShape::StringEnum { .. } => imports.string_enum = true,
            TypeShape::TaggedEnum { variants, .. } => {
                imports
                    .pydantic
                    .extend(["BaseModel", "ConfigDict", "Field"]);
                imports.typing.extend(["Literal", "TypeAlias"]);
                for variant in variants {
                    collect_field_imports(&variant.fields, &mut imports);
                }
            }
            TypeShape::Alias { target } => {
                imports.pydantic.insert("RootModel");
                collect_reference_imports(target, &mut imports);
            }
        }
    }
    imports
}

fn collect_field_imports(fields: &[FieldDef], imports: &mut ModelImports) {
    for field in fields {
        if field.constraints.literal.is_some() {
            imports.typing.insert("Literal");
        }
        collect_reference_imports(&field.ty, imports);
    }
}

fn collect_reference_imports(reference: &TypeRef, imports: &mut ModelImports) {
    match reference {
        TypeRef::DateTime => imports.datetime = true,
        TypeRef::Json => {
            imports.typing.insert("Any");
        }
        TypeRef::Option { item } | TypeRef::List { item } => {
            collect_reference_imports(item, imports);
        }
        TypeRef::Map { value } => collect_reference_imports(value, imports),
        TypeRef::Tuple { items } => {
            for item in items {
                collect_reference_imports(item, imports);
            }
        }
        TypeRef::Unit
        | TypeRef::Bool
        | TypeRef::Int { .. }
        | TypeRef::Float { .. }
        | TypeRef::String
        | TypeRef::Named { .. }
        | TypeRef::Bytes
        | TypeRef::FixedBytes { .. }
        | TypeRef::Buffer { .. } => {}
    }
}

fn begin_python_top_level(source: &mut String) {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    while !source.ends_with("\n\n\n") {
        source.push('\n');
    }
}

fn begin_python_alias(source: &mut String) {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    while !source.ends_with("\n\n") {
        source.push('\n');
    }
}

fn emit_python_type(source: &mut String, definition: &TypeDef, manifest: &Manifest) -> Result<()> {
    match &definition.shape {
        TypeShape::Struct { fields } => {
            begin_python_top_level(source);
            writeln!(source, "class {}(BaseModel):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
            if definition.docs.is_some() {
                source.push('\n');
            }
            emit_model_config(source);
            for field in fields {
                emit_python_field(source, field, manifest, "    ")?;
            }
        }
        TypeShape::StringEnum { variants } => {
            begin_python_top_level(source);
            writeln!(source, "class {}(StrEnum):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
            if definition.docs.is_some() && !variants.is_empty() {
                source.push('\n');
            }
            if variants.is_empty() {
                source.push_str("    pass\n");
            }
            for variant in variants {
                writeln!(
                    source,
                    "    {} = {}",
                    variant.rust_name,
                    py_string(&variant.wire_name)
                )?;
            }
        }
        TypeShape::TaggedEnum { tag, variants } => {
            for variant in variants {
                let name = tagged_variant_name(&definition.name, &variant.rust_name);
                begin_python_top_level(source);
                writeln!(source, "class {name}(BaseModel):")?;
                emit_python_doc(source, variant.docs.as_deref(), "    ")?;
                if variant.docs.is_some() {
                    source.push('\n');
                }
                emit_model_config(source);
                writeln!(
                    source,
                    "    {}: Literal[{}] = Field(default={}, alias={})",
                    safe_python_name(tag),
                    py_string(&variant.wire_name),
                    py_string(&variant.wire_name),
                    py_string(tag),
                )?;
                for field in &variant.fields {
                    emit_python_field(source, field, manifest, "    ")?;
                }
            }
            let names = variants
                .iter()
                .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name))
                .collect::<Vec<_>>()
                .join(" | ");
            begin_python_top_level(source);
            writeln!(source, "{}: TypeAlias = {}", definition.name, names)?;
        }
        TypeShape::Alias { target } => {
            begin_python_top_level(source);
            writeln!(
                source,
                "class {}(RootModel[{}]):\n    pass",
                definition.name,
                python_ref(target, manifest)?
            )?;
        }
    }
    Ok(())
}

fn emit_model_config(source: &mut String) {
    source.push_str(
        "    model_config = ConfigDict(\n        frozen=True,\n        strict=True,\n        populate_by_name=True,\n        extra=\"forbid\",\n        arbitrary_types_allowed=True,\n    )\n",
    );
}

fn emit_python_field(
    source: &mut String,
    field: &FieldDef,
    manifest: &Manifest,
    indent: &str,
) -> Result<()> {
    if let Some(docs) = field.docs.as_deref() {
        writeln!(source, "{indent}# {}", docs.replace('\n', " "))?;
    }
    let annotation = if let Some(literal) = &field.constraints.literal {
        format!("Literal[{}]", python_scalar(literal))
    } else {
        python_ref(&field.ty, manifest)?
    };
    let default = if field.required {
        "...".to_owned()
    } else if let Some(value) = &field.default {
        python_scalar(value)
    } else {
        "None".to_owned()
    };
    let mut options = vec![format!("default={default}")];
    if field.wire_name != field.rust_name {
        options.push(format!("alias={}", py_string(&field.wire_name)));
    }
    if let Some(value) = field.constraints.min_length {
        options.push(format!("min_length={value}"));
    }
    if let Some(value) = field.constraints.max_length {
        options.push(format!("max_length={value}"));
    }
    if let Some(value) = field.constraints.ge {
        options.push(format!("ge={value}"));
    }
    if let Some(value) = field.constraints.le {
        options.push(format!("le={value}"));
    }
    writeln!(
        source,
        "{indent}{}: {annotation} = Field({})",
        safe_python_name(&field.rust_name),
        options.join(", ")
    )?;
    Ok(())
}

fn python_api(manifest: &Manifest) -> Result<String> {
    let has_constants = !manifest.constants.is_empty();
    let needs_buffer_adapter = manifest
        .functions
        .iter()
        .any(|function| reference_uses_buffer(&function.returns))
        || manifest.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| reference_uses_buffer(&method.returns))
        })
        || manifest
            .constants
            .iter()
            .any(|constant| reference_uses_buffer(&constant.ty));
    let needs_type_adapter = manifest
        .functions
        .iter()
        .any(|function| !matches!(function.returns, TypeRef::Unit))
        || manifest.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| !matches!(method.returns, TypeRef::Unit))
        })
        || manifest
            .constants
            .iter()
            .any(|constant| !is_plain_python_constant(&constant.ty));
    let references = python_api_references(manifest);
    let mut source = String::from("from __future__ import annotations\n");
    let uses_datetime = references
        .iter()
        .any(|reference| reference_contains(reference, &|item| matches!(item, TypeRef::DateTime)));
    let mut typing_imports = BTreeSet::new();
    if references
        .iter()
        .any(|reference| reference_contains(reference, &|item| matches!(item, TypeRef::Json)))
    {
        typing_imports.insert("Any");
    }
    if has_constants {
        typing_imports.insert("Final");
    }
    if uses_datetime || !typing_imports.is_empty() {
        source.push('\n');
    }
    if uses_datetime {
        source.push_str("from datetime import datetime\n");
    }
    if !typing_imports.is_empty() {
        writeln!(
            source,
            "from typing import {}",
            typing_imports.into_iter().collect::<Vec<_>>().join(", ")
        )?;
    }
    let mut pydantic_imports = Vec::new();
    if needs_buffer_adapter {
        pydantic_imports.push("ConfigDict");
    }
    if needs_type_adapter {
        pydantic_imports.push("TypeAdapter");
    }
    if !pydantic_imports.is_empty() {
        writeln!(
            source,
            "\nfrom pydantic import {}",
            pydantic_imports.join(", ")
        )?;
    }
    let model_names = python_model_names(manifest);
    let runtime_imports = python_runtime_imports(manifest);
    if !model_names.is_empty() || !runtime_imports.is_empty() {
        source.push('\n');
    }
    if !model_names.is_empty() {
        source.push_str("from .models import (\n");
        for name in model_names {
            writeln!(source, "    {name},")?;
        }
        source.push_str(")\n");
    }
    if !runtime_imports.is_empty() {
        source.push_str("from .runtime import (\n");
        for name in runtime_imports {
            writeln!(source, "    {name},")?;
        }
        source.push_str(")\n");
    }

    for error in &manifest.errors {
        begin_python_top_level(&mut source);
        writeln!(source, "class {}(RuntimeError):", error.name)?;
        emit_python_doc(&mut source, error.docs.as_deref(), "    ")?;
        if error.docs.is_some() {
            source.push('\n');
        }
        source.push_str("    def __init__(self, code: str, message: str) -> None:\n        super().__init__(message)\n        self.code = code\n");
    }
    for function in &manifest.functions {
        emit_python_function(&mut source, function, manifest, None)?;
    }
    for resource in &manifest.resources {
        emit_python_resource(&mut source, resource, manifest)?;
    }
    for constant in &manifest.constants {
        begin_python_top_level(&mut source);
        let value = python_json(&constant.value);
        let ty = python_ref(&constant.ty, manifest)?;
        if is_plain_python_constant(&constant.ty) {
            writeln!(source, "{}: Final[{ty}] = {value}", constant.host_name)?;
        } else {
            write!(source, "{}: Final[{ty}] = ", constant.host_name)?;
            emit_python_validation(
                &mut source,
                &constant.ty,
                manifest,
                &format!(
                    "restore_host({value}, {})",
                    python_spec(&constant.ty, manifest)?
                ),
                "",
            )?;
        }
    }
    Ok(source)
}

fn python_runtime(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "from __future__ import annotations\n\nfrom datetime import date, datetime\nfrom typing import Any\n\n",
    );
    if uses_buffer(manifest) {
        source.push_str("import numpy as np\n");
    }
    source.push_str("from pydantic import BaseModel\n");
    writeln!(
        source,
        "\nfrom . import {} as native  # type: ignore[attr-defined]",
        manifest.module_name
    )?;
    emit_python_adapters(&mut source, uses_buffer(manifest));
    begin_python_top_level(&mut source);
    writeln!(source, "native_schemas: dict[str, Any] = {{")?;
    for definition in &manifest.types {
        writeln!(
            source,
            "    {}: {},",
            py_string(&definition.name),
            python_named_spec(definition, manifest)?
        )?;
    }
    source.push_str("}\n");
    Ok(source)
}

fn python_api_references(manifest: &Manifest) -> Vec<&TypeRef> {
    let mut references = Vec::new();
    for function in &manifest.functions {
        references.extend(function.params.iter().map(|param| &param.ty));
        references.push(&function.returns);
    }
    for resource in &manifest.resources {
        for constructor in &resource.constructors {
            references.extend(constructor.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            references.extend(method.params.iter().map(|param| &param.ty));
            references.push(&method.returns);
        }
    }
    references.extend(manifest.constants.iter().map(|constant| &constant.ty));
    references
}

fn reference_contains(reference: &TypeRef, predicate: &impl Fn(&TypeRef) -> bool) -> bool {
    if predicate(reference) {
        return true;
    }
    match reference {
        TypeRef::Option { item } | TypeRef::List { item } => reference_contains(item, predicate),
        TypeRef::Map { value } => reference_contains(value, predicate),
        TypeRef::Tuple { items } => items.iter().any(|item| reference_contains(item, predicate)),
        _ => false,
    }
}

fn python_runtime_imports(manifest: &Manifest) -> Vec<&'static str> {
    let has_calls = !manifest.functions.is_empty() || !manifest.resources.is_empty();
    let has_params = manifest
        .functions
        .iter()
        .any(|function| !function.params.is_empty())
        || manifest.resources.iter().any(|resource| {
            resource
                .constructors
                .iter()
                .any(|constructor| !constructor.params.is_empty())
                || resource
                    .methods
                    .iter()
                    .any(|method| !method.params.is_empty())
        });
    let restores_values = manifest
        .functions
        .iter()
        .any(|function| !matches!(function.returns, TypeRef::Unit))
        || manifest.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| !matches!(method.returns, TypeRef::Unit))
        })
        || manifest
            .constants
            .iter()
            .any(|constant| !is_plain_python_constant(&constant.ty));
    let translates_errors = manifest
        .functions
        .iter()
        .any(|function| function.error.is_some())
        || manifest.resources.iter().any(|resource| {
            resource
                .constructors
                .iter()
                .any(|constructor| constructor.error.is_some())
                || resource.methods.iter().any(|method| method.error.is_some())
        });
    let mut imports = Vec::new();
    if has_calls {
        imports.push("native");
    }
    if translates_errors {
        imports.push("native_error");
    }
    if has_params {
        imports.push("prepare_host");
    }
    if restores_values {
        imports.push("restore_host");
    }
    imports
}

fn reference_uses_buffer(reference: &TypeRef) -> bool {
    let mut buffers = BTreeSet::new();
    collect_buffers(reference, &mut buffers);
    !buffers.is_empty()
}

fn emit_python_adapters(source: &mut String, with_buffers: bool) {
    source.push_str(PYTHON_ADAPTERS_START);
    if with_buffers {
        source.push_str("    if isinstance(value, np.ndarray):\n        return value.tolist()\n");
    }
    source.push_str(PYTHON_ADAPTERS_RESTORE);
    if with_buffers {
        source.push_str(
            "    if kind == \"buffer\":\n        return np.asarray(value, dtype=spec[1])\n",
        );
    }
    source.push_str(PYTHON_ADAPTERS_END);
}

const PYTHON_ADAPTERS_START: &str = r#"

def prepare_host(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return prepare_host(value.model_dump(mode="python", by_alias=True))
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return list(value)
"#;

const PYTHON_ADAPTERS_RESTORE: &str = r#"    if isinstance(value, dict):
        return {key: prepare_host(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [prepare_host(item) for item in value]
    return value


def restore_host(value: Any, spec: Any) -> Any:
    if value is None or spec is None:
        return value
    kind = spec[0]
    if kind == "bytes":
        return bytes(value)
"#;

const PYTHON_ADAPTERS_END: &str = r#"    if kind == "list":
        return [restore_host(item, spec[1]) for item in value]
    if kind == "map":
        return {key: restore_host(item, spec[1]) for key, item in value.items()}
    if kind == "tuple":
        return tuple(
            restore_host(item, item_spec) for item, item_spec in zip(value, spec[1])
        )
    if kind == "named":
        return restore_host(value, native_schemas.get(spec[1]))
    if kind == "alias":
        return restore_host(value, spec[1])
    if kind == "struct":
        return {
            key: restore_host(item, spec[1].get(key)) for key, item in value.items()
        }
    if kind == "tagged":
        fields = spec[2].get(value.get(spec[1]), {})
        return {key: restore_host(item, fields.get(key)) for key, item in value.items()}
    return value


def native_error(error: RuntimeError, error_type: type[RuntimeError]) -> RuntimeError:
    if len(error.args) == 2:
        return error_type(str(error.args[0]), str(error.args[1]))
    return error
"#;

fn emit_python_function(
    source: &mut String,
    function: &FunctionDef,
    manifest: &Manifest,
    receiver: Option<&str>,
) -> Result<()> {
    let params = function
        .params
        .iter()
        .map(|param| python_param(param, manifest))
        .collect::<Result<Vec<_>>>()?;
    let call_params = function
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>();
    let return_type = python_ref(&function.returns, manifest)?;
    let (indent, first_param, call) = if receiver.is_some() {
        (
            "    ",
            Some("self"),
            format!("self.native_resource.{}", function.host_name),
        )
    } else {
        ("", None, format!("native.{}", function.host_name))
    };
    if receiver.is_some() {
        source.push('\n');
    } else {
        begin_python_top_level(source);
    }
    emit_python_signature(
        source,
        indent,
        &function.rust_name,
        first_param,
        &params,
        &return_type,
    )?;
    emit_python_doc(source, function.docs.as_deref(), &format!("{indent}    "))?;
    if function.error.is_some() {
        writeln!(source, "{indent}    try:")?;
        emit_python_call(
            source,
            &format!("{indent}        result = "),
            &format!("{indent}        "),
            &call,
            &call_params,
        )?;
        writeln!(source, "{indent}    except RuntimeError as error:")?;
        let error_name = error_name(function.error.as_ref(), manifest)?;
        writeln!(
            source,
            "{indent}        raise native_error(error, {error_name}) from None"
        )?;
    } else {
        emit_python_call(
            source,
            &format!("{indent}    result = "),
            &format!("{indent}    "),
            &call,
            &call_params,
        )?;
    }
    if matches!(function.returns, TypeRef::Unit) {
        writeln!(source, "{indent}    return None")?;
    } else {
        write!(source, "{indent}    return ")?;
        emit_python_validation(
            source,
            &function.returns,
            manifest,
            &format!(
                "restore_host(result, {})",
                python_spec(&function.returns, manifest)?
            ),
            &format!("{indent}    "),
        )?;
    }
    Ok(())
}

fn emit_python_signature(
    source: &mut String,
    indent: &str,
    name: &str,
    first_param: Option<&str>,
    params: &[String],
    return_type: &str,
) -> Result<()> {
    let all_params = first_param
        .into_iter()
        .map(str::to_owned)
        .chain(params.iter().cloned())
        .collect::<Vec<_>>();
    let compact = format!(
        "{indent}def {name}({}) -> {return_type}:",
        all_params.join(", ")
    );
    if compact.chars().count() <= 88 {
        writeln!(source, "{compact}")?;
        return Ok(());
    }
    writeln!(source, "{indent}def {name}(")?;
    for param in all_params {
        writeln!(source, "{indent}    {param},")?;
    }
    writeln!(source, "{indent}) -> {return_type}:")?;
    Ok(())
}

fn emit_python_call(
    source: &mut String,
    prefix: &str,
    indent: &str,
    callable: &str,
    args: &[String],
) -> Result<()> {
    let compact = format!("{prefix}{callable}({})", args.join(", "));
    if compact.chars().count() <= 88 {
        writeln!(source, "{compact}")?;
        return Ok(());
    }
    writeln!(source, "{prefix}{callable}(")?;
    for arg in args {
        writeln!(source, "{indent}    {arg},")?;
    }
    writeln!(source, "{indent})")?;
    Ok(())
}

fn emit_python_validation(
    source: &mut String,
    reference: &TypeRef,
    manifest: &Manifest,
    value: &str,
    indent: &str,
) -> Result<()> {
    let annotation = python_adapter_type(reference, manifest)?;
    let mut buffers = BTreeSet::new();
    collect_buffers(reference, &mut buffers);
    if buffers.is_empty() {
        writeln!(source, "TypeAdapter({annotation}).validate_python(")?;
    } else {
        writeln!(source, "TypeAdapter(")?;
        writeln!(source, "{indent}    {annotation},")?;
        writeln!(
            source,
            "{indent}    config=ConfigDict(arbitrary_types_allowed=True),"
        )?;
        writeln!(source, "{indent}).validate_python(")?;
    }
    writeln!(source, "{indent}    {value},")?;
    writeln!(source, "{indent}    strict=False,")?;
    writeln!(source, "{indent})")?;
    Ok(())
}

fn emit_python_resource(
    source: &mut String,
    resource: &ResourceDef,
    manifest: &Manifest,
) -> Result<()> {
    let constructor = resource
        .constructors
        .iter()
        .find(|item| item.rust_name == "new")
        .or_else(|| resource.constructors.first())
        .context("resource has no constructor")?;
    begin_python_top_level(source);
    writeln!(source, "class {}:", resource.name)?;
    emit_python_doc(source, resource.docs.as_deref(), "    ")?;
    if resource.docs.is_some() {
        source.push('\n');
    }
    let params = constructor
        .params
        .iter()
        .map(|param| python_param(param, manifest))
        .collect::<Result<Vec<_>>>()?;
    let calls = constructor
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>();
    emit_python_signature(source, "    ", "__init__", Some("self"), &params, "None")?;
    let native_call = format!("native.{}", resource.name);
    if constructor.error.is_some() {
        writeln!(source, "        try:")?;
        emit_python_call(
            source,
            "            self.native_resource = ",
            "            ",
            &native_call,
            &calls,
        )?;
        writeln!(source, "        except RuntimeError as error:")?;
        writeln!(
            source,
            "            raise native_error(error, {}) from None",
            error_name(constructor.error.as_ref(), manifest)?
        )?;
    } else {
        emit_python_call(
            source,
            "        self.native_resource = ",
            "        ",
            &native_call,
            &calls,
        )?;
    }
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        let params = factory
            .params
            .iter()
            .map(|param| python_param(param, manifest))
            .collect::<Result<Vec<_>>>()?;
        let calls = factory
            .params
            .iter()
            .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
            .collect::<Vec<_>>();
        source.push_str("\n    @classmethod\n");
        emit_python_signature(
            source,
            "    ",
            &factory.rust_name,
            Some("cls"),
            &params,
            &resource.name,
        )?;
        writeln!(source, "        value = cls.__new__(cls)")?;
        let native_call = format!("native.{}.{}", resource.name, factory.host_name);
        if factory.error.is_some() {
            writeln!(source, "        try:")?;
            emit_python_call(
                source,
                "            value.native_resource = ",
                "            ",
                &native_call,
                &calls,
            )?;
            writeln!(source, "        except RuntimeError as error:")?;
            writeln!(
                source,
                "            raise native_error(error, {}) from None",
                error_name(factory.error.as_ref(), manifest)?
            )?;
        } else {
            emit_python_call(
                source,
                "        value.native_resource = ",
                "        ",
                &native_call,
                &calls,
            )?;
        }
        writeln!(source, "        return value")?;
    }
    for method in &resource.methods {
        let function = FunctionDef {
            rust_name: method.rust_name.clone(),
            host_name: method.host_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_python_function(source, &function, manifest, Some(&resource.name))?;
    }
    source.push_str("\n    def close(self) -> None:\n        self.native_resource.close()\n");
    Ok(())
}

fn python_init(manifest: &Manifest) -> String {
    let mut model_names = python_model_names(manifest);
    let mut api_names = manifest
        .errors
        .iter()
        .map(|item| item.name.clone())
        .chain(manifest.functions.iter().map(|item| item.rust_name.clone()))
        .chain(manifest.resources.iter().map(|item| item.name.clone()))
        .chain(manifest.constants.iter().map(|item| item.host_name.clone()))
        .collect::<Vec<_>>();
    model_names.sort();
    api_names.sort();
    let mut source = String::from("\"\"\"Generated from the Rust application API.\"\"\"\n\n");
    if !api_names.is_empty() {
        source.push_str("from .api import (\n");
        for name in &api_names {
            writeln!(source, "    {name},").unwrap();
        }
        source.push_str(")\n");
    }
    if !model_names.is_empty() {
        source.push_str("from .models import (\n");
        for name in &model_names {
            writeln!(source, "    {name},").unwrap();
        }
        source.push_str(")\n");
    }
    let mut all = model_names;
    all.extend(api_names);
    source.push_str("\n__all__ = [\n");
    for name in all {
        writeln!(source, "    {},", py_string(&name)).unwrap();
    }
    source.push_str("]\n");
    source
}

fn python_ref(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "None".into(),
        TypeRef::Bool => "bool".into(),
        TypeRef::Int { .. } => "int".into(),
        TypeRef::Float { .. } => "float".into(),
        TypeRef::String => "str".into(),
        TypeRef::DateTime => "datetime".into(),
        TypeRef::Json => "Any".into(),
        TypeRef::Option { item } => format!("{} | None", python_ref(item, manifest)?),
        TypeRef::List { item } => format!("list[{}]", python_ref(item, manifest)?),
        TypeRef::Map { value } => format!("dict[str, {}]", python_ref(value, manifest)?),
        TypeRef::Tuple { items } => format!(
            "tuple[{}]",
            items
                .iter()
                .map(|item| python_ref(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => type_name(identity, manifest)?.to_owned(),
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "bytes".into(),
        TypeRef::Buffer { element } => buffer_name(*element).into(),
    })
}

fn python_adapter_type(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    if matches!(reference, TypeRef::Unit) {
        Ok("type(None)".into())
    } else {
        python_ref(reference, manifest)
    }
}

fn python_param(param: &ParamDef, manifest: &Manifest) -> Result<String> {
    Ok(format!(
        "{}: {}",
        safe_python_name(&param.rust_name),
        python_ref(&param.ty, manifest)?
    ))
}

fn python_spec(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    Ok(match reference {
        TypeRef::Option { item } => python_spec(item, manifest)?,
        TypeRef::List { item } => format!("(\"list\", {})", python_spec(item, manifest)?),
        TypeRef::Map { value } => format!("(\"map\", {})", python_spec(value, manifest)?),
        TypeRef::Tuple { items } => format!(
            "(\"tuple\", ({}))",
            items
                .iter()
                .map(|item| python_spec(item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            format!("(\"named\", {})", py_string(type_name(identity, manifest)?))
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "(\"bytes\",)".into(),
        TypeRef::Buffer { element } => {
            format!("(\"buffer\", {})", py_string(python_numpy_scalar(*element)))
        }
        _ => "None".into(),
    })
}

fn python_named_spec(definition: &TypeDef, manifest: &Manifest) -> Result<String> {
    Ok(match &definition.shape {
        TypeShape::Struct { fields } => format!(
            "(\"struct\", {{{}}})",
            fields
                .iter()
                .map(|field| Ok(format!(
                    "{}: {}",
                    py_string(&field.wire_name),
                    python_spec(&field.ty, manifest)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::TaggedEnum { tag, variants } => format!(
            "(\"tagged\", {}, {{{}}})",
            py_string(tag),
            variants
                .iter()
                .map(|variant| Ok(format!(
                    "{}: {{{}}}",
                    py_string(&variant.wire_name),
                    variant
                        .fields
                        .iter()
                        .map(|field| Ok(format!(
                            "{}: {}",
                            py_string(&field.wire_name),
                            python_spec(&field.ty, manifest)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::Alias { target } => {
            format!("(\"alias\", {})", python_spec(target, manifest)?)
        }
        TypeShape::StringEnum { .. } => "None".into(),
    })
}

fn python_model_names(manifest: &Manifest) -> Vec<String> {
    let mut names = Vec::new();
    for definition in &manifest.types {
        names.push(definition.name.clone());
        if let TypeShape::TaggedEnum { variants, .. } = &definition.shape {
            names.extend(
                variants
                    .iter()
                    .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name)),
            );
        }
    }
    names.extend(
        buffer_elements(manifest)
            .into_iter()
            .map(|element| buffer_name(element).to_owned()),
    );
    names.sort();
    names.dedup();
    names
}

fn python_scalar(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(value) => if *value { "True" } else { "False" }.into(),
        ScalarValue::I64(value) => value.to_string(),
        ScalarValue::String(value) => py_string(value),
    }
}

fn python_json(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => py_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", py_string(key), python_json(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn is_plain_python_constant(reference: &TypeRef) -> bool {
    matches!(
        reference,
        TypeRef::Unit
            | TypeRef::Bool
            | TypeRef::Int { .. }
            | TypeRef::Float { .. }
            | TypeRef::String
            | TypeRef::Json
    )
}

fn emit_python_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs {
        let escaped = py_string(docs);
        let escaped = &escaped[1..escaped.len() - 1];
        if !docs.contains('\n') && indent.chars().count() + escaped.chars().count() + 6 <= 88 {
            writeln!(source, "{indent}\"\"\"{escaped}\"\"\"")?;
        } else {
            let lines = wrap_python_doc(docs, 84usize.saturating_sub(indent.len()));
            write!(source, "{indent}\"\"\"")?;
            for (index, line) in lines.iter().enumerate() {
                if index > 0 && !line.is_empty() {
                    write!(source, "{indent}")?;
                }
                let escaped = py_string(line);
                write!(source, "{}", &escaped[1..escaped.len() - 1])?;
                source.push('\n');
            }
            writeln!(source, "{indent}\"\"\"")?;
        }
    }
    Ok(())
}

fn wrap_python_doc(value: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    for original in value.lines() {
        let mut remaining = original;
        while py_string(remaining).chars().count().saturating_sub(2) > width {
            let mut boundary = None;
            for (index, character) in remaining.char_indices() {
                let prefix = &remaining[..index];
                if py_string(prefix).chars().count().saturating_sub(2) > width {
                    break;
                }
                if character.is_whitespace() {
                    boundary = Some(index);
                }
            }
            let boundary = boundary.unwrap_or_else(|| {
                remaining
                    .char_indices()
                    .take_while(|(index, _)| {
                        py_string(&remaining[..*index])
                            .chars()
                            .count()
                            .saturating_sub(2)
                            <= width
                    })
                    .map(|(index, _)| index)
                    .last()
                    .unwrap_or(remaining.len())
            });
            if boundary == 0 || boundary >= remaining.len() {
                break;
            }
            result.push(remaining[..boundary].trim_end().to_owned());
            remaining = remaining[boundary..].trim_start();
        }
        result.push(remaining.to_owned());
    }
    if value.ends_with('\n') {
        result.push(String::new());
    }
    result
}

fn safe_python_name(value: &str) -> String {
    if matches!(
        value,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    ) {
        format!("{value}_value")
    } else {
        value.to_owned()
    }
}

fn py_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

pub(super) fn buffer_name(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "UInt8Buffer",
        BufferElement::I8 => "Int8Buffer",
        BufferElement::U16 => "UInt16Buffer",
        BufferElement::I16 => "Int16Buffer",
        BufferElement::U32 => "UInt32Buffer",
        BufferElement::I32 => "Int32Buffer",
        BufferElement::U64 => "UInt64Buffer",
        BufferElement::I64 => "Int64Buffer",
        BufferElement::F32 => "Float32Buffer",
        BufferElement::F64 => "Float64Buffer",
    }
}

fn python_numpy_scalar(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "uint8",
        BufferElement::I8 => "int8",
        BufferElement::U16 => "uint16",
        BufferElement::I16 => "int16",
        BufferElement::U32 => "uint32",
        BufferElement::I32 => "int32",
        BufferElement::U64 => "uint64",
        BufferElement::I64 => "int64",
        BufferElement::F32 => "float32",
        BufferElement::F64 => "float64",
    }
}

#[cfg(test)]
mod tests {
    use rspyts::ir::{Manifest, TypeRef};
    use serde_json::json;

    use super::{python_api, python_models, python_project_file, python_runtime};

    #[test]
    fn generated_project_declares_only_required_python_packages() {
        let standard = python_project_file("example", "1.0.0", false);
        assert!(standard.contains("requires = [\"setuptools>=77\"]"));
        assert!(standard.contains("dependencies = [\"pydantic>=2,<3\"]"));
        assert!(!standard.contains("numpy"));
        assert!(!standard.contains("wheel"));
        assert!(!standard.contains("*.pyi"));

        let buffered = python_project_file("example", "1.0.0", true);
        assert!(buffered.contains("dependencies = [\"pydantic>=2,<3\", \"numpy>=2,<3\"]"));
    }

    #[test]
    fn generated_models_import_only_the_types_they_use() {
        let manifest = manifest_with_types(json!([
            {
                "owner": "example",
                "id": "example::Message",
                "name": "Message",
                "docs": "A message.",
                "shape": {
                    "kind": "struct",
                    "fields": [{
                        "rustName": "text",
                        "wireName": "text",
                        "docs": null,
                        "ty": {"kind": "string"},
                        "required": true,
                        "default": null,
                        "constraints": {
                            "literal": null,
                            "minLength": null,
                            "maxLength": null,
                            "ge": null,
                            "le": null
                        }
                    }]
                }
            }
        ]));

        let generated = python_models(&manifest).expect("models generate");
        assert!(generated.contains("from pydantic import BaseModel, ConfigDict, Field"));
        assert!(generated.contains("    \"\"\"A message.\"\"\""));
        assert!(!generated.contains("from datetime"));
        assert!(!generated.contains("from enum"));
        assert!(!generated.contains("from typing"));
        assert!(!generated.contains("RootModel"));
    }

    #[test]
    fn generated_api_keeps_boundary_code_in_the_runtime_module() {
        let mut manifest = manifest_with_types(json!([]));
        manifest.functions.push(rspyts::ir::FunctionDef {
            rust_name: "ping".into(),
            host_name: "ping".into(),
            docs: None,
            params: Vec::new(),
            returns: TypeRef::Unit,
            error: None,
        });

        let standard = python_api(&manifest).expect("standard API generates");
        assert!(!standard.contains("import numpy"));
        assert!(!standard.contains("np."));
        assert!(!standard.contains("TypeAdapter"));
        assert!(!standard.contains("ConfigDict"));
        assert!(!standard.contains("def prepare_host"));
        assert!(standard.contains("from .runtime import"));

        let standard_runtime = python_runtime(&manifest).expect("standard runtime generates");
        assert!(!standard_runtime.contains("import numpy"));
        assert!(standard_runtime.contains("def prepare_host"));

        manifest.functions[0].returns = TypeRef::Buffer {
            element: rspyts::ir::BufferElement::U32,
        };
        let buffered = python_api(&manifest).expect("buffered API generates");
        assert!(!buffered.contains("import numpy as np"));
        assert!(buffered.contains("TypeAdapter"));
        assert!(buffered.contains("ConfigDict"));
        assert!(!buffered.contains("return np.asarray"));

        let buffered_runtime = python_runtime(&manifest).expect("buffered runtime generates");
        assert!(buffered_runtime.contains("import numpy as np"));
        assert!(buffered_runtime.contains("return np.asarray"));
    }

    fn manifest_with_types(types: serde_json::Value) -> Manifest {
        serde_json::from_value(json!({
            "irVersion": 1,
            "packageName": "example",
            "packageVersion": "1.0.0",
            "moduleName": "native",
            "types": types,
            "errors": [],
            "functions": [],
            "resources": [],
            "constants": []
        }))
        .expect("valid test manifest")
    }
}
