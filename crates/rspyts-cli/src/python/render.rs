//! Python syntax primitives shared by model and API generation.
//!
//! This module owns all host spelling decisions: type references, imports,
//! runtime schemas, literals, identifier escaping, and docstring wrapping.
//! Keeping those decisions here prevents API and model emitters from drifting.

use super::*;
use crate::documentation::{
    CallableDocumentation, Documentation, contextualize_error_description,
    remove_rustdoc_link_brackets,
};

// Type and schema rendering ------------------------------------------------

/// Render a contract type as a Python annotation in the current namespace.
pub(crate) fn python_ref(reference: &TypeRef, context: &PythonContext<'_>) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "None".into(),
        TypeRef::Bool => "bool".into(),
        TypeRef::Int { .. } => "int".into(),
        TypeRef::Float { .. } => "float".into(),
        TypeRef::String => "str".into(),
        TypeRef::DateTime => "datetime".into(),
        TypeRef::Json => "Any".into(),
        TypeRef::Option { item } => format!("{} | None", python_ref(item, context)?),
        TypeRef::List { item } => format!("list[{}]", python_ref(item, context)?),
        TypeRef::Map { value } => format!("dict[str, {}]", python_ref(value, context)?),
        TypeRef::Tuple { items } => format!(
            "tuple[{}]",
            items
                .iter()
                .map(|item| python_ref(item, context))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => python_named_ref(identity, context)?,
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "bytes".into(),
        TypeRef::Buffer { element } => buffer_name(*element).into(),
    })
}

/// Render the annotation passed to Pydantic's runtime adapter.
pub(crate) fn python_adapter_type(
    reference: &TypeRef,
    context: &PythonContext<'_>,
) -> Result<String> {
    if matches!(reference, TypeRef::Unit) {
        Ok("type(None)".into())
    } else {
        python_ref(reference, context)
    }
}

/// Render a named and annotated Python parameter.
pub(crate) fn python_param(param: &ParamDef, context: &PythonContext<'_>) -> Result<String> {
    Ok(format!(
        "{}: {}",
        safe_python_name(&param.rust_name),
        python_ref(&param.ty, context)?
    ))
}

/// Render the compact runtime restoration schema for a type reference.
pub(crate) fn python_spec(reference: &TypeRef) -> Result<String> {
    Ok(match reference {
        TypeRef::Option { item } => python_spec(item)?,
        TypeRef::List { item } => format!("(\"list\", {})", python_spec(item)?),
        TypeRef::Map { value } => format!("(\"map\", {})", python_spec(value)?),
        TypeRef::Tuple { items } => format!(
            "(\"tuple\", ({}))",
            items
                .iter()
                .map(python_spec)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            format!("(\"named\", {})", py_string(&definition_key(identity)))
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "(\"bytes\",)".into(),
        TypeRef::Buffer { element } => {
            format!("(\"buffer\", {})", py_string(python_numpy_scalar(*element)))
        }
        _ => "None".into(),
    })
}

/// Render the runtime restoration schema for a named model definition.
pub(crate) fn python_named_spec(definition: &TypeDef) -> Result<String> {
    Ok(match &definition.shape {
        TypeShape::Struct { fields } => format!(
            "(\"struct\", {{{}}})",
            fields
                .iter()
                .map(|field| Ok(format!(
                    "{}: {}",
                    py_string(&field.wire_name),
                    python_spec(&field.ty)?
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
                            python_spec(&field.ty)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::Alias { target } => {
            format!("(\"alias\", {})", python_spec(target)?)
        }
        TypeShape::StringEnum { .. } => "None".into(),
    })
}

// Import and name resolution -----------------------------------------------

/// Return sorted model and buffer names declared in one namespace.
pub(crate) fn python_model_names(items: &NamespaceItems<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for definition in &items.types {
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
        namespace_buffers(items)
            .into_iter()
            .map(|element| buffer_name(element).to_owned()),
    );
    names.sort();
    names.dedup();
    names
}

/// Return numeric buffer types directly referenced by one namespace.
pub(crate) fn namespace_buffers(items: &NamespaceItems<'_>) -> BTreeSet<BufferElement> {
    let mut buffers = BTreeSet::new();
    for reference in namespace_refs(items) {
        collect_buffers(reference, &mut buffers);
    }
    buffers
}

/// Return external model namespaces imported by a generated models module.
pub(crate) fn python_model_imports(
    items: &NamespaceItems<'_>,
    context: &PythonContext<'_>,
) -> Result<BTreeSet<Namespace>> {
    let references = items
        .types
        .iter()
        .flat_map(|definition| type_refs(definition))
        .collect::<Vec<_>>();
    python_api_model_imports(&references, context)
}

/// Return external model namespaces referenced by API type references.
pub(crate) fn python_api_model_imports(
    references: &[&TypeRef],
    context: &PythonContext<'_>,
) -> Result<BTreeSet<Namespace>> {
    let mut imports = BTreeSet::new();
    for reference in references {
        let mut identities = Vec::new();
        named_identities(reference, &mut identities);
        for identity in identities {
            let namespace = type_namespace(identity, context.manifest)?;
            if namespace != *context.namespace {
                imports.insert(namespace);
            }
        }
    }
    Ok(imports)
}

/// Return external API modules containing errors used in this namespace.
pub(crate) fn python_error_imports(
    items: &NamespaceItems<'_>,
    context: &PythonContext<'_>,
) -> Result<BTreeSet<String>> {
    let mut identities = items
        .functions
        .iter()
        .filter_map(|function| function.error.as_ref())
        .collect::<Vec<_>>();
    identities.extend(items.resources.iter().flat_map(|resource| {
        resource
            .constructors
            .iter()
            .filter_map(|constructor| constructor.error.as_ref())
            .chain(
                resource
                    .methods
                    .iter()
                    .filter_map(|method| method.error.as_ref()),
            )
    }));
    let mut imports = BTreeSet::new();
    for identity in identities {
        let definition = error_definition(identity, context.manifest)?;
        let namespace = context
            .manifest
            .namespace(&definition.owner, &definition.rust_module);
        if namespace != *context.namespace {
            imports.insert(python_module(context.package, &namespace, "api"));
        }
    }
    Ok(imports)
}

/// Render a local or namespace-qualified named model reference.
pub(crate) fn python_named_ref(
    identity: &rspyts::ir::DefinitionId,
    context: &PythonContext<'_>,
) -> Result<String> {
    let definition = type_definition(identity, context.manifest)?;
    let namespace = context
        .manifest
        .namespace(&definition.owner, &definition.rust_module);
    if namespace == *context.namespace {
        Ok(definition.name.clone())
    } else {
        Ok(format!(
            "{}.{}",
            python_model_alias(&namespace, context),
            definition.name
        ))
    }
}

/// Return the deterministic import alias for a model namespace.
pub(crate) fn python_model_alias(namespace: &Namespace, context: &PythonContext<'_>) -> String {
    let index = namespaces(context.manifest)
        .keys()
        .position(|candidate| candidate == namespace)
        .expect("referenced namespace belongs to the manifest");
    format!("_rspyts_models_{index}")
}

/// Render a local or namespace-qualified generated error reference.
pub(crate) fn python_error_ref(
    identity: Option<&rspyts::ir::DefinitionId>,
    context: &PythonContext<'_>,
) -> Result<String> {
    let identity = identity.context("missing error identity")?;
    let definition = error_definition(identity, context.manifest)?;
    let namespace = context
        .manifest
        .namespace(&definition.owner, &definition.rust_module);
    if namespace == *context.namespace {
        Ok(definition.name.clone())
    } else {
        Ok(format!(
            "{}.{}",
            python_module(context.package, &namespace, "api"),
            definition.name
        ))
    }
}

/// Render an absolute generated Python module path.
pub(crate) fn python_module(package: &str, namespace: &Namespace, leaf: &str) -> String {
    package
        .split('.')
        .map(str::to_owned)
        .chain(namespace.python_segments())
        .chain([leaf.to_owned()])
        .collect::<Vec<_>>()
        .join(".")
}

// Literals and documentation -----------------------------------------------

/// Render a scalar default or literal constraint as Python source.
pub(crate) fn python_scalar(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(value) => if *value { "True" } else { "False" }.into(),
        ScalarValue::I64(value) => value.to_string(),
        ScalarValue::String(value) => py_string(value),
    }
}

/// Render arbitrary JSON as deterministic Python source.
pub(crate) fn python_json(value: &Value) -> String {
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

/// Return whether a constant can be assigned without runtime restoration.
pub(crate) fn is_plain_python_constant(reference: &TypeRef) -> bool {
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

/// Emit a multiline Python docstring with stable wrapping and indentation.
pub(crate) fn emit_python_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs.filter(|docs| !docs.trim().is_empty()) {
        emit_python_docstring(source, &remove_rustdoc_link_brackets(docs), indent)?;
    }
    Ok(())
}

/// Emit a Google-style docstring for a generated callable.
pub(crate) fn emit_python_callable_doc(
    source: &mut String,
    callable: &CallableDocumentation<'_>,
    context: &PythonContext<'_>,
    indent: &str,
) -> Result<()> {
    let documentation = Documentation::parse(callable.docs);
    let summary = if documentation.summary.is_empty() {
        &callable.fallback_summary
    } else {
        &documentation.summary
    };
    let mut blocks = vec![remove_rustdoc_link_brackets(summary)];

    let mut notes = remove_rustdoc_link_brackets(&documentation.notes);
    append_python_paragraph(
        &mut notes,
        "The implementation executes in the compiled Rust extension.",
    );
    blocks.push(python_doc_section("Notes", &notes));

    if !callable.params.is_empty() {
        let mut entries = Vec::new();
        for param in callable.params {
            let name = safe_python_name(&param.rust_name);
            let description = documentation
                .parameters
                .get(&param.rust_name)
                .or_else(|| documentation.parameters.get(&param.host_name))
                .filter(|description| !description.is_empty())
                .map(|description| remove_rustdoc_link_brackets(description))
                .unwrap_or(format!(
                    "Value typed as ``{}``.",
                    python_ref(&param.ty, context)?
                ));
            entries.push(format!(
                "    {name}: {}",
                indent_python_continuation(&description)
            ));
        }
        blocks.push(format!("Args:\n{}", entries.join("\n")));
    }

    if let Some(returns) = callable
        .returns
        .filter(|returns| !matches!(returns, TypeRef::Unit))
    {
        let description = if documentation.returns.is_empty() {
            format!("A value typed as ``{}``.", python_ref(returns, context)?)
        } else {
            remove_rustdoc_link_brackets(&documentation.returns)
        };
        blocks.push(python_doc_section("Returns", &description));
    }

    if let Some(error) = callable.error {
        let error_name = python_error_ref(Some(error), context)?;
        let description = if documentation.errors.is_empty() {
            "Raised when the Rust implementation reports an error.".to_owned()
        } else {
            contextualize_error_description(&remove_rustdoc_link_brackets(&documentation.errors))
        };
        blocks.push(format!(
            "Raises:\n    {error_name}: {}",
            indent_python_continuation(&description)
        ));
    }

    if !documentation.examples.is_empty() {
        blocks.push(python_doc_section(
            "Examples",
            &remove_rustdoc_link_brackets(&documentation.examples),
        ));
    }

    emit_python_docstring(source, &blocks.join("\n\n"), indent)
}

/// Emit the delimiters and escaped body shared by every Python docstring.
fn emit_python_docstring(source: &mut String, docs: &str, indent: &str) -> Result<()> {
    writeln!(source, "{indent}\"\"\"")?;
    for line in wrap_python_doc(docs, 84usize.saturating_sub(indent.len())) {
        if line.is_empty() {
            source.push('\n');
        } else {
            let escaped = py_string(&line);
            writeln!(source, "{indent}{}", &escaped[1..escaped.len() - 1])?;
        }
    }
    writeln!(source, "{indent}\"\"\"")?;
    Ok(())
}

/// Indent every line in one Google-style documentation section.
fn python_doc_section(name: &str, value: &str) -> String {
    format!(
        "{name}:\n{}",
        value
            .lines()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Append a distinct paragraph to optional generated notes.
fn append_python_paragraph(target: &mut String, value: &str) {
    if !target.is_empty() {
        target.push_str("\n\n");
    }
    target.push_str(value);
}

/// Align continuation lines beneath a Google-style entry description.
fn indent_python_continuation(value: &str) -> String {
    value.replace('\n', "\n        ")
}

/// Wrap prose to a target width without breaking words unnecessarily.
pub(crate) fn wrap_python_doc(value: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    for original in value.lines() {
        if original.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let leading = original.len() - original.trim_start().len();
        let first_prefix = &original[..leading];
        let continuation_width = if original.trim_start().contains(": ") {
            leading + 4
        } else {
            leading
        };
        let continuation_prefix = " ".repeat(continuation_width);
        let mut prefix = first_prefix.to_owned();
        let mut remaining = original.trim_start();
        while prefix.chars().count() + py_string(remaining).chars().count().saturating_sub(2)
            > width
        {
            let available = width.saturating_sub(prefix.chars().count());
            let mut boundary = None;
            for (index, character) in remaining.char_indices() {
                let prefix = &remaining[..index];
                if py_string(prefix).chars().count().saturating_sub(2) > available {
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
                            <= available
                    })
                    .map(|(index, _)| index)
                    .last()
                    .unwrap_or(remaining.len())
            });
            if boundary == 0 || boundary >= remaining.len() {
                break;
            }
            result.push(format!("{prefix}{}", remaining[..boundary].trim_end()));
            remaining = remaining[boundary..].trim_start();
            prefix.clone_from(&continuation_prefix);
        }
        result.push(format!("{prefix}{remaining}"));
    }
    if value.ends_with('\n') {
        result.push(String::new());
    }
    result
}

/// Normalize a Rust identifier into a legal, non-keyword Python name.
pub(crate) fn safe_python_name(value: &str) -> String {
    let mut name = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if index == 0 && character.is_ascii_digit() {
                name.push_str("value_");
            }
            name.push(character);
        } else {
            name.push('_');
        }
    }
    if name.is_empty() {
        name.push_str("value");
    }
    if matches!(
        name.as_str(),
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
            | "mro"
    ) {
        name.push_str("_value");
    }
    name
}

/// Quote a Python string literal with deterministic JSON-compatible escaping.
pub(crate) fn py_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

/// Return the generated NumPy array alias for a contract buffer element.
pub(crate) fn buffer_name(element: BufferElement) -> &'static str {
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

/// Return the NumPy scalar spelling for a contract buffer element.
pub(crate) fn python_numpy_scalar(element: BufferElement) -> &'static str {
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
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            package_name: "test".to_owned(),
            package_version: "0.0.0".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: vec![rspyts::ir::ErrorDef {
                owner: rspyts::ir::CargoPackageId::new("test"),
                rust_module: "test".to_owned(),
                id: "test::ExampleError".to_owned(),
                name: "ExampleError".to_owned(),
                docs: None,
            }],
            functions: Vec::new(),
            resources: Vec::new(),
            constants: Vec::new(),
        }
    }

    #[test]
    fn ordinary_docstrings_always_use_multiline_delimiters() {
        let mut source = String::new();
        emit_python_doc(&mut source, Some("Short documentation."), "    ")
            .expect("render docstring");

        assert_eq!(source, "    \"\"\"\n    Short documentation.\n    \"\"\"\n");
        assert!(!source.contains("\"\"\"Short"));
    }

    #[test]
    fn callable_docstrings_use_google_sections() {
        let manifest = manifest();
        let namespace = Namespace::root();
        let context = PythonContext {
            manifest: &manifest,
            package: "test",
            namespace: &namespace,
        };
        let params = vec![ParamDef {
            rust_name: "value".to_owned(),
            host_name: "value".to_owned(),
            ty: TypeRef::String,
        }];
        let error = rspyts::ir::DefinitionId::new("test", "test::ExampleError");
        let mut source = String::new();

        emit_python_callable_doc(
            &mut source,
            &CallableDocumentation {
                docs: Some(
                    "Process a value.\n\nRetains ordering.\n\n# Arguments\n\n- `value` - Value to process.\n\n# Returns\n\nThe processed value.\n\n# Errors\n\nReturns `ExampleError` when validation fails.",
                ),
                fallback_summary: "Fallback.".to_owned(),
                params: &params,
                returns: Some(&TypeRef::String),
                error: Some(&error),
            },
            &context,
            "    ",
        )
        .expect("render callable docstring");

        assert!(source.starts_with("    \"\"\"\n    Process a value.\n"));
        assert!(source.contains("    Notes:\n        Retains ordering."));
        assert!(source.contains("    Args:\n        value: Value to process."));
        assert!(source.contains("    Returns:\n        The processed value."));
        assert!(source.contains(
            "    Raises:\n        ExampleError: The Rust implementation returns `ExampleError` when"
        ));
        assert!(!source.contains("# Errors"));
        assert!(source.lines().all(|line| !line.ends_with(' ')));
    }
}
