use super::*;

pub(super) fn python_api(
    items: &NamespaceItems<'_>,
    context: &PythonContext<'_>,
) -> Result<String> {
    let has_constants = !items.constants.is_empty();
    let needs_buffer_adapter = items
        .functions
        .iter()
        .any(|function| reference_uses_buffer(&function.returns))
        || items.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| reference_uses_buffer(&method.returns))
        })
        || items
            .constants
            .iter()
            .any(|constant| reference_uses_buffer(&constant.ty));
    let needs_type_adapter = items
        .functions
        .iter()
        .any(|function| !matches!(function.returns, TypeRef::Unit))
        || items.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| !matches!(method.returns, TypeRef::Unit))
        })
        || items
            .constants
            .iter()
            .any(|constant| !is_plain_python_constant(&constant.ty));
    let references = python_api_references(items);
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
    let model_names = python_model_names(items);
    let runtime_imports = python_runtime_imports(items);
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
    for namespace in python_api_model_imports(&references, context)? {
        writeln!(
            source,
            "import {} as {}",
            python_module(context.package, &namespace, "models"),
            python_model_alias(&namespace, context)
        )?;
    }
    for import in python_error_imports(items, context)? {
        writeln!(source, "import {import}")?;
    }
    if !runtime_imports.is_empty() {
        writeln!(source, "from {}.runtime import (", context.package)?;
        for name in runtime_imports {
            writeln!(source, "    {name},")?;
        }
        source.push_str(")\n");
    }

    for error in &items.errors {
        begin_python_top_level(&mut source);
        writeln!(source, "class {}(RuntimeError):", error.name)?;
        emit_python_doc(&mut source, error.docs.as_deref(), "    ")?;
        if error.docs.is_some() {
            source.push('\n');
        }
        source.push_str("    def __init__(self, code: str, message: str) -> None:\n        super().__init__(message)\n        self.code = code\n");
    }
    for function in &items.functions {
        emit_python_function(&mut source, function, context, None)?;
    }
    for resource in &items.resources {
        emit_python_resource(&mut source, resource, context)?;
    }
    for constant in &items.constants {
        begin_python_top_level(&mut source);
        let value = python_json(&constant.value);
        let ty = python_ref(&constant.ty, context)?;
        if is_plain_python_constant(&constant.ty) {
            writeln!(source, "{}: Final[{ty}] = {value}", constant.host_name)?;
        } else {
            write!(source, "{}: Final[{ty}] = ", constant.host_name)?;
            emit_python_validation(
                &mut source,
                &constant.ty,
                context,
                &format!("restore_host({value}, {})", python_spec(&constant.ty)?),
                "",
            )?;
        }
    }
    Ok(source)
}

pub(super) fn python_runtime(manifest: &Manifest) -> Result<String> {
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
            py_string(&definition_key(&definition.identity())),
            python_named_spec(definition)?
        )?;
    }
    source.push_str("}\n");
    Ok(source)
}

fn python_api_references<'a>(items: &'a NamespaceItems<'a>) -> Vec<&'a TypeRef> {
    let mut references = Vec::new();
    for function in &items.functions {
        references.extend(function.params.iter().map(|param| &param.ty));
        references.push(&function.returns);
    }
    for resource in &items.resources {
        for constructor in &resource.constructors {
            references.extend(constructor.params.iter().map(|param| &param.ty));
        }
        for method in &resource.methods {
            references.extend(method.params.iter().map(|param| &param.ty));
            references.push(&method.returns);
        }
    }
    references.extend(items.constants.iter().map(|constant| &constant.ty));
    references
}

fn python_runtime_imports(items: &NamespaceItems<'_>) -> Vec<&'static str> {
    let has_calls = !items.functions.is_empty() || !items.resources.is_empty();
    let has_params = items
        .functions
        .iter()
        .any(|function| !function.params.is_empty())
        || items.resources.iter().any(|resource| {
            resource
                .constructors
                .iter()
                .any(|constructor| !constructor.params.is_empty())
                || resource
                    .methods
                    .iter()
                    .any(|method| !method.params.is_empty())
        });
    let restores_values = items
        .functions
        .iter()
        .any(|function| !matches!(function.returns, TypeRef::Unit))
        || items.resources.iter().any(|resource| {
            resource
                .methods
                .iter()
                .any(|method| !matches!(method.returns, TypeRef::Unit))
        })
        || items
            .constants
            .iter()
            .any(|constant| !is_plain_python_constant(&constant.ty));
    let translates_errors = items
        .functions
        .iter()
        .any(|function| function.error.is_some())
        || items.resources.iter().any(|resource| {
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
    context: &PythonContext<'_>,
    receiver: Option<&str>,
) -> Result<()> {
    let params = function
        .params
        .iter()
        .map(|param| python_param(param, context))
        .collect::<Result<Vec<_>>>()?;
    let call_params = function
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>();
    let mut result_name = "native_result".to_owned();
    let parameter_names = function
        .params
        .iter()
        .map(|param| safe_python_name(&param.rust_name))
        .collect::<BTreeSet<_>>();
    while parameter_names.contains(&result_name) {
        result_name.push_str("_value");
    }
    let return_type = python_ref(&function.returns, context)?;
    let (indent, first_param, call) = if receiver.is_some() {
        (
            "    ",
            Some("self"),
            format!("self.native_resource.{}", function.host_name),
        )
    } else {
        (
            "",
            None,
            format!("getattr(native, {})", py_string(&function.native_name)),
        )
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
            &format!("{indent}        {result_name} = "),
            &format!("{indent}        "),
            &call,
            &call_params,
        )?;
        writeln!(source, "{indent}    except RuntimeError as error:")?;
        let error_name = python_error_ref(function.error.as_ref(), context)?;
        writeln!(
            source,
            "{indent}        raise native_error(error, {error_name}) from None"
        )?;
    } else {
        emit_python_call(
            source,
            &format!("{indent}    {result_name} = "),
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
            context,
            &format!(
                "restore_host({result_name}, {})",
                python_spec(&function.returns)?
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
    context: &PythonContext<'_>,
    value: &str,
    indent: &str,
) -> Result<()> {
    let annotation = python_adapter_type(reference, context)?;
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
    context: &PythonContext<'_>,
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
        .map(|param| python_param(param, context))
        .collect::<Result<Vec<_>>>()?;
    let calls = constructor
        .params
        .iter()
        .map(|param| format!("prepare_host({})", safe_python_name(&param.rust_name)))
        .collect::<Vec<_>>();
    emit_python_signature(source, "    ", "__init__", Some("self"), &params, "None")?;
    let native_call = format!("getattr(native, {})", py_string(&resource.native_name));
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
            python_error_ref(constructor.error.as_ref(), context)?
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
            .map(|param| python_param(param, context))
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
        let native_call = format!(
            "getattr(native, {}).{}",
            py_string(&resource.native_name),
            factory.host_name
        );
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
                python_error_ref(factory.error.as_ref(), context)?
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
            owner: resource.owner.clone(),
            rust_module: resource.rust_module.clone(),
            rust_name: method.rust_name.clone(),
            host_name: method.host_name.clone(),
            native_name: resource.native_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_python_function(source, &function, context, Some(&resource.name))?;
    }
    source.push_str("\n    def close(self) -> None:\n        self.native_resource.close()\n");
    Ok(())
}
