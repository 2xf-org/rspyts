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
    write(&package.join("api.py"), &python_api(manifest)?)?;
    write(&package.join("__init__.py"), &python_init(manifest))?;
    write(&package.join("py.typed"), "")?;
    write(
        &package.join(format!("{}.pyi", manifest.module_name)),
        &python_native_stub(manifest),
    )?;

    let distribution = project.python_package.replace('.', "-");
    let dependencies = if uses_buffer(manifest) {
        "dependencies = [\"pydantic>=2,<3\", \"numpy>=2,<3\"]"
    } else {
        "dependencies = [\"pydantic>=2,<3\"]"
    };
    let pyproject = format!(
        "[build-system]\nrequires = [\"setuptools>=77\", \"wheel>=0.45\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = {}\nversion = {}\nrequires-python = \">=3.11\"\n{}\n\n[tool.setuptools.packages.find]\nwhere = [\".\"]\n\n[tool.setuptools.package-data]\n\"*\" = [\"*.so\", \"*.pyd\", \"*.pyi\", \"py.typed\"]\n",
        py_string(&distribution),
        py_string(&manifest.package_version),
        dependencies,
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

fn python_models(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "from __future__ import annotations\n\nfrom datetime import datetime\nfrom enum import StrEnum\nfrom typing import Annotated, Any, Literal, TypeAlias\n\nfrom pydantic import BaseModel, ConfigDict, Field, RootModel\n",
    );
    let buffers = buffer_elements(manifest);
    if !buffers.is_empty() {
        source.push_str("from pydantic.functional_serializers import PlainSerializer\nfrom pydantic.functional_validators import BeforeValidator\nimport numpy as np\nfrom numpy.typing import NDArray\n");
    }
    source.push('\n');
    for element in buffers {
        let name = buffer_name(element);
        let scalar = python_numpy_scalar(element);
        writeln!(
            source,
            "{name}: TypeAlias = Annotated[NDArray[np.{scalar}], BeforeValidator(lambda value: np.asarray(value, dtype=np.{scalar})), PlainSerializer(lambda value: value.tolist(), return_type=list)]"
        )?;
    }
    if uses_buffer(manifest) {
        source.push('\n');
    }

    for definition in &manifest.types {
        emit_python_type(&mut source, definition, manifest)?;
    }
    for definition in &manifest.types {
        match definition.shape {
            TypeShape::Struct { .. } | TypeShape::Alias { .. } => {
                writeln!(source, "{}.model_rebuild()", definition.name)?;
            }
            TypeShape::TaggedEnum { ref variants, .. } => {
                for variant in variants {
                    writeln!(
                        source,
                        "{}.model_rebuild()",
                        tagged_variant_name(&definition.name, &variant.rust_name)
                    )?;
                }
            }
            TypeShape::StringEnum { .. } => {}
        }
    }
    Ok(source)
}

fn emit_python_type(source: &mut String, definition: &TypeDef, manifest: &Manifest) -> Result<()> {
    match &definition.shape {
        TypeShape::Struct { fields } => {
            writeln!(source, "\nclass {}(BaseModel):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
            source.push_str("    model_config = ConfigDict(frozen=True, strict=True, populate_by_name=True, extra=\"forbid\", arbitrary_types_allowed=True)\n");
            if fields.is_empty() {
                source.push_str("    pass\n");
            }
            for field in fields {
                emit_python_field(source, field, manifest, "    ")?;
            }
        }
        TypeShape::StringEnum { variants } => {
            writeln!(source, "\nclass {}(StrEnum):", definition.name)?;
            emit_python_doc(source, definition.docs.as_deref(), "    ")?;
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
                writeln!(source, "\nclass {name}(BaseModel):")?;
                emit_python_doc(source, variant.docs.as_deref(), "    ")?;
                source.push_str("    model_config = ConfigDict(frozen=True, strict=True, populate_by_name=True, extra=\"forbid\", arbitrary_types_allowed=True)\n");
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
            writeln!(source, "\n{}: TypeAlias = {}", definition.name, names)?;
        }
        TypeShape::Alias { target } => {
            writeln!(
                source,
                "\nclass {}(RootModel[{}]):\n    pass",
                definition.name,
                python_ref(target, manifest)?
            )?;
        }
    }
    Ok(())
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
    let mut source = String::from(
        "from __future__ import annotations\n\nfrom datetime import date, datetime\nfrom typing import Any, Final\n\nfrom pydantic import BaseModel, ConfigDict, TypeAdapter\n",
    );
    if uses_buffer(manifest) {
        source.push_str("import numpy as np\n");
    }
    let model_names = python_model_names(manifest);
    if !model_names.is_empty() {
        writeln!(source, "\nfrom .models import {}", model_names.join(", "))?;
    }
    writeln!(source, "from . import {} as native\n", manifest.module_name)?;
    source.push_str(PYTHON_ADAPTERS);
    writeln!(source, "\nnative_schemas: dict[str, Any] = {{")?;
    for definition in &manifest.types {
        writeln!(
            source,
            "    {}: {},",
            py_string(&definition.name),
            python_named_spec(definition, manifest)?
        )?;
    }
    source.push_str("}\n");

    for error in &manifest.errors {
        writeln!(source, "\nclass {}(RuntimeError):", error.name)?;
        emit_python_doc(&mut source, error.docs.as_deref(), "    ")?;
        source.push_str("    def __init__(self, code: str, message: str) -> None:\n        super().__init__(message)\n        self.code = code\n\n");
    }
    for function in &manifest.functions {
        emit_python_function(&mut source, function, manifest, None)?;
    }
    for resource in &manifest.resources {
        emit_python_resource(&mut source, resource, manifest)?;
    }
    for constant in &manifest.constants {
        let value = python_json(&constant.value);
        let ty = python_ref(&constant.ty, manifest)?;
        if is_plain_python_constant(&constant.ty) {
            writeln!(source, "\n{}: Final[{ty}] = {value}", constant.host_name)?;
        } else {
            writeln!(
                source,
                "\n{}: Final[{ty}] = {}.validate_python(restore_host({value}, {}))",
                constant.host_name,
                type_adapter(&constant.ty, manifest)?,
                python_spec(&constant.ty, manifest)?
            )?;
        }
    }
    Ok(source)
}

const PYTHON_ADAPTERS: &str = r#"
def prepare_host(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return prepare_host(value.model_dump(mode="python", by_alias=True))
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return list(value)
    if "np" in globals() and isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, dict):
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
    if kind == "buffer":
        return np.asarray(value, dtype=spec[1])
    if kind == "list":
        return [restore_host(item, spec[1]) for item in value]
    if kind == "map":
        return {key: restore_host(item, spec[1]) for key, item in value.items()}
    if kind == "tuple":
        return tuple(restore_host(item, item_spec) for item, item_spec in zip(value, spec[1]))
    if kind == "named":
        return restore_host(value, native_schemas.get(spec[1]))
    if kind == "alias":
        return restore_host(value, spec[1])
    if kind == "struct":
        return {key: restore_host(item, spec[1].get(key)) for key, item in value.items()}
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
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let call_params = function
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>()
        .join(", ");
    let return_type = python_ref(&function.returns, manifest)?;
    let (indent, signature, call) = if receiver.is_some() {
        (
            "    ",
            format!(
                "    def {}(self{}{}) -> {return_type}:",
                function.rust_name,
                if params.is_empty() { "" } else { ", " },
                params
            ),
            format!("self.native_resource.{}({call_params})", function.host_name),
        )
    } else {
        (
            "",
            format!("def {}({params}) -> {return_type}:", function.rust_name),
            format!("native.{}({call_params})", function.host_name),
        )
    };
    writeln!(source, "\n{signature}")?;
    emit_python_doc(source, function.docs.as_deref(), &format!("{indent}    "))?;
    if function.error.is_some() {
        writeln!(source, "{indent}    try:")?;
        writeln!(source, "{indent}        result = {call}")?;
        writeln!(source, "{indent}    except RuntimeError as error:")?;
        let error_name = error_name(function.error.as_ref(), manifest)?;
        writeln!(
            source,
            "{indent}        raise native_error(error, {error_name}) from None"
        )?;
    } else {
        writeln!(source, "{indent}    result = {call}")?;
    }
    if matches!(function.returns, TypeRef::Unit) {
        writeln!(source, "{indent}    return None")?;
    } else {
        writeln!(
            source,
            "{indent}    return {}.validate_python(restore_host(result, {}))",
            type_adapter(&function.returns, manifest)?,
            python_spec(&function.returns, manifest)?
        )?;
    }
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
    writeln!(source, "\nclass {}:", resource.name)?;
    emit_python_doc(source, resource.docs.as_deref(), "    ")?;
    let params = constructor
        .params
        .iter()
        .map(|param| python_param(param, manifest))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let calls = constructor
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        source,
        "    def __init__(self{}{}) -> None:",
        if params.is_empty() { "" } else { ", " },
        params
    )?;
    let native_call = format!("native.{}({calls})", resource.name);
    if constructor.error.is_some() {
        writeln!(source, "        try:")?;
        writeln!(source, "            self.native_resource = {native_call}")?;
        writeln!(source, "        except RuntimeError as error:")?;
        writeln!(
            source,
            "            raise native_error(error, {}) from None",
            error_name(constructor.error.as_ref(), manifest)?
        )?;
    } else {
        writeln!(source, "        self.native_resource = {native_call}")?;
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
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let calls = factory
            .params
            .iter()
            .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            source,
            "\n    @classmethod\n    def {}(cls, {params}) -> {}:",
            factory.rust_name, resource.name
        )?;
        writeln!(source, "        value = cls.__new__(cls)")?;
        let native_call = format!("native.{}.{}({calls})", resource.name, factory.host_name);
        if factory.error.is_some() {
            writeln!(source, "        try:")?;
            writeln!(source, "            value.native_resource = {native_call}")?;
            writeln!(source, "        except RuntimeError as error:")?;
            writeln!(
                source,
                "            raise native_error(error, {}) from None",
                error_name(factory.error.as_ref(), manifest)?
            )?;
        } else {
            writeln!(source, "        value.native_resource = {native_call}")?;
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
    if !model_names.is_empty() {
        writeln!(source, "from .models import {}", model_names.join(", ")).unwrap();
    }
    if !api_names.is_empty() {
        writeln!(source, "from .api import {}", api_names.join(", ")).unwrap();
    }
    let mut all = model_names;
    all.extend(api_names);
    writeln!(
        source,
        "\n__all__ = [{}]",
        all.iter()
            .map(|item| py_string(item))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    source
}

fn python_native_stub(manifest: &Manifest) -> String {
    let mut source = String::from("from typing import Any\n\n");
    for function in &manifest.functions {
        let params = function
            .params
            .iter()
            .map(|param| format!("{}: Any", safe_python_name(&param.rust_name)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(source, "def {}({params}) -> Any: ...", function.host_name).unwrap();
    }
    for resource in &manifest.resources {
        writeln!(source, "\nclass {}:", resource.name).unwrap();
        source.push_str("    def __init__(self, *args: Any) -> None: ...\n");
        for method in &resource.methods {
            writeln!(
                source,
                "    def {}(self, *args: Any) -> Any: ...",
                method.host_name
            )
            .unwrap();
        }
        source.push_str("    def close(self) -> None: ...\n");
    }
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

pub(super) fn type_adapter(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    let annotation = python_adapter_type(reference, manifest)?;
    let mut buffers = BTreeSet::new();
    collect_buffers(reference, &mut buffers);
    if buffers.is_empty() {
        Ok(format!("TypeAdapter({annotation})"))
    } else {
        Ok(format!(
            "TypeAdapter({annotation}, config=ConfigDict(arbitrary_types_allowed=True))"
        ))
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
        writeln!(source, "{indent}{}", py_string(docs))?;
    }
    Ok(())
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
