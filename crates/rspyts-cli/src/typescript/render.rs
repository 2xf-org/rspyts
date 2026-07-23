//! TypeScript syntax primitives shared by declarations and API generation.
//!
//! Type spelling, namespace-relative imports, runtime schemas, JSON literals,
//! identifier quoting, and documentation comments are centralized here so all
//! generated modules follow one deterministic policy.

use super::*;
use crate::documentation::{
    CallableDocumentation, CallableReturn, Documentation, contextualize_error_description,
    remove_rustdoc_link_brackets,
};

// Type and import rendering ------------------------------------------------

/// Render a contract type in the current TypeScript namespace.
pub(crate) fn type_ref(reference: &TypeRef, context: &TypeScriptContext<'_>) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "null".into(),
        TypeRef::Bool => "boolean".into(),
        TypeRef::Int { bits: 64, .. } => "bigint".into(),
        TypeRef::Int { .. } | TypeRef::Float { .. } => "number".into(),
        TypeRef::String | TypeRef::DateTime => "string".into(),
        TypeRef::Json => "JsonValue".into(),
        TypeRef::Option { item } => format!("{} | null", type_ref(item, context)?),
        TypeRef::List { item } => format!("ReadonlyArray<{}>", type_ref(item, context)?),
        TypeRef::Map { value } => {
            format!("Readonly<Record<string, {}>>", type_ref(value, context)?)
        }
        TypeRef::Tuple { items } => format!(
            "readonly [{}]",
            items
                .iter()
                .map(|item| type_ref(item, context))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => typescript_named_ref(identity, context)?,
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "Uint8Array".into(),
        TypeRef::Buffer { element } => typescript_buffer_name(*element).into(),
    })
}

/// Render a callable return type, spelling contract unit as `void`.
pub(crate) fn return_type_ref(
    reference: &TypeRef,
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    if matches!(reference, TypeRef::Unit) {
        return Ok("void".into());
    }
    type_ref(reference, context)
}

/// Return external model namespaces referenced by one namespace.
pub(crate) fn typescript_type_imports(
    items: &NamespaceItems<'_>,
    context: &TypeScriptContext<'_>,
) -> Result<BTreeSet<Namespace>> {
    let mut imports = BTreeSet::new();
    for reference in namespace_refs(items) {
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

/// Return external API namespaces containing referenced error classes.
pub(crate) fn typescript_error_imports(
    items: &NamespaceItems<'_>,
    context: &TypeScriptContext<'_>,
) -> Result<BTreeSet<Namespace>> {
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
            imports.insert(namespace);
        }
    }
    Ok(imports)
}

/// Render a local or namespace-qualified named model reference.
pub(crate) fn typescript_named_ref(
    identity: &rspyts::ir::DefinitionId,
    context: &TypeScriptContext<'_>,
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
            namespace_alias(&namespace),
            definition.name
        ))
    }
}

/// Return the deterministic API import alias corresponding to a namespace.
pub(crate) fn api_namespace_alias(namespace: &Namespace) -> String {
    format!("api_{}", &namespace_alias(namespace)["types_".len()..])
}

/// Render a local or namespace-qualified generated error reference.
pub(crate) fn typescript_error_ref(
    identity: Option<&rspyts::ir::DefinitionId>,
    context: &TypeScriptContext<'_>,
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
            api_namespace_alias(&namespace),
            definition.name
        ))
    }
}

/// Return a deterministic and identifier-safe model import alias.
pub(crate) fn namespace_alias(namespace: &Namespace) -> String {
    let segments = namespace.typescript_segments();
    let suffix = if segments.is_empty() {
        "root".to_owned()
    } else {
        segments
            .join("_")
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("types_{suffix}")
}

/// Render an ECMAScript-relative path between generated namespace modules.
pub(crate) fn typescript_namespace_path(from: &Namespace, to: &Namespace, leaf: &str) -> String {
    let from = from.typescript_segments();
    let to = to.typescript_segments();
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_owned(); from.len().saturating_sub(shared)];
    parts.extend(to.into_iter().skip(shared));
    parts.push(leaf.to_owned());
    let path = parts.join("/");
    if path.starts_with('.') {
        path
    } else {
        format!("./{path}")
    }
}

/// Render the relative runtime-module path for a generated namespace.
pub(crate) fn typescript_runtime_path(namespace: &Namespace) -> String {
    let depth = namespace.typescript_segments().len();
    if depth == 0 {
        "./runtime.js".to_owned()
    } else {
        format!("{}runtime.js", "../".repeat(depth))
    }
}

/// Render a comma-separated TypeScript parameter list.
pub(crate) fn typescript_params(
    params: &[ParamDef],
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    params
        .iter()
        .map(|param| {
            Ok(format!(
                "{}: {}",
                param.host_name,
                type_ref(&param.ty, context)?
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|items| items.join(", "))
}

// Runtime schema and value rendering ---------------------------------------

/// Render the compact runtime restoration schema for a type reference.
pub(crate) fn typescript_spec(reference: &TypeRef) -> Result<String> {
    Ok(match reference {
        TypeRef::Json => "[\"json\"]".into(),
        TypeRef::Option { item } => typescript_spec(item)?,
        TypeRef::List { item } => format!("[\"list\", {}]", typescript_spec(item)?),
        TypeRef::Map { value } => format!("[\"map\", {}]", typescript_spec(value)?),
        TypeRef::Tuple { items } => format!(
            "[\"tuple\", [{}]]",
            items
                .iter()
                .map(typescript_spec)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            format!("[\"named\", {}]", ts_string(&definition_key(identity)))
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => "[\"bytes\"]".into(),
        TypeRef::Buffer { element } => {
            format!("[\"buffer\", {}]", ts_string(buffer_key(*element)))
        }
        _ => "null".into(),
    })
}

/// Render the runtime restoration schema for a named model definition.
pub(crate) fn typescript_named_spec(definition: &TypeDef) -> Result<String> {
    Ok(match &definition.shape {
        TypeShape::Struct { fields } => format!(
            "[\"struct\", {{{}}}]",
            fields
                .iter()
                .map(|field| Ok(format!(
                    "{}: {}",
                    ts_property(&field.wire_name),
                    typescript_spec(&field.ty)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::TaggedEnum { tag, variants } => format!(
            "[\"tagged\", {}, {{{}}}]",
            ts_string(tag),
            variants
                .iter()
                .map(|variant| Ok(format!(
                    "{}: {{{}}}",
                    ts_property(&variant.wire_name),
                    variant
                        .fields
                        .iter()
                        .map(|field| Ok(format!(
                            "{}: {}",
                            ts_property(&field.wire_name),
                            typescript_spec(&field.ty)?
                        )))
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeShape::Alias { target } => {
            format!("[\"alias\", {}]", typescript_spec(target)?)
        }
        TypeShape::StringEnum { .. } => "null".into(),
    })
}

/// Render a JSON value according to its declared contract type.
pub(crate) fn typescript_value(
    value: &Value,
    reference: &TypeRef,
    manifest: &Manifest,
) -> Result<String> {
    if value.is_null() {
        return Ok("null".into());
    }
    Ok(match reference {
        TypeRef::Int { bits: 64, .. } => format!(
            "{}n",
            value
                .as_u64()
                .map(|item| item.to_string())
                .or_else(|| value.as_i64().map(|item| item.to_string()))
                .context("invalid 64-bit constant")?
        ),
        TypeRef::Option { item } => typescript_value(value, item, manifest)?,
        TypeRef::List { item } => format!(
            "[{}]",
            value
                .as_array()
                .context("invalid list constant")?
                .iter()
                .map(|value| typescript_value(value, item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Map { value: item } => format!(
            "{{{}}}",
            value
                .as_object()
                .context("invalid map constant")?
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}: {}",
                    ts_property(key),
                    typescript_value(value, item, manifest)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Tuple { items } => format!(
            "[{}]",
            value
                .as_array()
                .context("invalid tuple constant")?
                .iter()
                .zip(items)
                .map(|(value, item)| typescript_value(value, item, manifest))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        TypeRef::Named { identity } => {
            let definition = type_definition(identity, manifest)?;
            typescript_named_value(value, definition, manifest)?
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } | TypeRef::Buffer { .. } => {
            serde_json::to_string(value)?
        }
        TypeRef::Json => typescript_json_value(value)?,
        _ => serde_json::to_string(value)?,
    })
}

/// Render an untyped JSON value, preserving integers outside JS's safe range.
fn typescript_json_value(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null | Value::Bool(_) | Value::String(_) => serde_json::to_string(value)?,
        Value::Number(number) => {
            const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
            if let Some(value) = number.as_u64()
                && value > MAX_SAFE_INTEGER
            {
                format!("{value}n")
            } else if let Some(value) = number.as_i64()
                && value.unsigned_abs() > MAX_SAFE_INTEGER
            {
                format!("{value}n")
            } else {
                number.to_string()
            }
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(typescript_json_value)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}: {}",
                    ts_property(key),
                    typescript_json_value(value)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
    })
}

/// Render a value through the shape of a resolved named definition.
pub(crate) fn typescript_named_value(
    value: &Value,
    definition: &TypeDef,
    manifest: &Manifest,
) -> Result<String> {
    match &definition.shape {
        TypeShape::Alias { target } => typescript_value(value, target, manifest),
        TypeShape::StringEnum { .. } => Ok(serde_json::to_string(value)?),
        TypeShape::Struct { fields } => typescript_object_value(value, fields, manifest),
        TypeShape::TaggedEnum { tag, variants } => {
            let object = value.as_object().context("invalid tagged enum constant")?;
            let tag_value = object
                .get(tag)
                .and_then(Value::as_str)
                .context("tagged enum constant has no tag")?;
            let variant = variants
                .iter()
                .find(|variant| variant.wire_name == tag_value)
                .context("unknown tagged enum constant variant")?;
            let mut fields = variant.fields.clone();
            fields.push(FieldDef {
                rust_name: tag.clone(),
                wire_name: tag.clone(),
                docs: None,
                ty: TypeRef::String,
                required: true,
                default: None,
                constraints: FieldConstraints::default(),
            });
            typescript_object_value(value, &fields, manifest)
        }
    }
}

/// Render an object with deterministic property order and field typing.
pub(crate) fn typescript_object_value(
    value: &Value,
    fields: &[FieldDef],
    manifest: &Manifest,
) -> Result<String> {
    let object = value.as_object().context("invalid object constant")?;
    Ok(format!(
        "{{{}}}",
        object
            .iter()
            .map(|(key, value)| {
                let field = fields
                    .iter()
                    .find(|field| field.wire_name == *key)
                    .context("constant has an unknown field")?;
                Ok(format!(
                    "{}: {}",
                    ts_property(key),
                    typescript_value(value, &field.ty, manifest)?
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ))
}

// Documentation and lexical primitives ------------------------------------

/// Emit an escaped, multiline TSDoc comment.
pub(crate) fn emit_ts_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs.filter(|docs| !docs.trim().is_empty()) {
        emit_ts_doc_block(source, &remove_rustdoc_link_brackets(docs), indent)?;
    }
    Ok(())
}

/// Emit TSDoc for a generated function, constructor, or resource method.
pub(crate) fn emit_ts_callable_doc(
    source: &mut String,
    callable: &CallableDocumentation<'_>,
    context: &TypeScriptContext<'_>,
    indent: &str,
) -> Result<()> {
    let documentation = Documentation::parse(callable.docs);
    let summary = if documentation.summary.is_empty() {
        &callable.fallback_summary
    } else {
        &documentation.summary
    };
    let mut prose_blocks = vec![typescript_documentation_text(summary, context)];

    let mut remarks = typescript_documentation_text(&documentation.notes, context);
    append_ts_paragraph(
        &mut remarks,
        "The implementation executes in the generated WebAssembly module.",
    );
    prose_blocks.push(format!("@remarks\n{remarks}"));

    let mut tags = Vec::new();

    for param in callable.params {
        let description = documentation
            .parameters
            .get(&param.rust_name)
            .or_else(|| documentation.parameters.get(&param.host_name))
            .filter(|description| !description.is_empty())
            .map(|description| typescript_documentation_text(description, context))
            .unwrap_or(format!(
                "Value typed as `{}`.",
                type_ref(&param.ty, context)?
            ));
        tags.push(format!(
            "@param {} - {}",
            param.host_name,
            indent_ts_tag_continuation(&description)
        ));
    }

    let rendered_return = match callable.returns {
        CallableReturn::Omitted | CallableReturn::Contract(TypeRef::Unit) => None,
        CallableReturn::Contract(returns) => Some(type_ref(returns, context)?),
        CallableReturn::Resource(name) => Some(name.to_owned()),
    };
    if let Some(rendered_return) = rendered_return {
        let description = if documentation.returns.is_empty() {
            format!("A value typed as `{rendered_return}`.")
        } else {
            typescript_documentation_text(&documentation.returns, context)
        };
        tags.push(format!(
            "@returns {}",
            indent_ts_tag_continuation(&description)
        ));
    }

    if let Some(error) = callable.error {
        let error_name = typescript_error_ref(Some(error), context)?;
        let description = if documentation.errors.is_empty() {
            "Thrown when the Rust implementation reports an error.".to_owned()
        } else {
            contextualize_error_description(&typescript_documentation_text(
                &documentation.errors,
                context,
            ))
        };
        tags.push(format!(
            "@throws {{@link {error_name}}} - {}",
            indent_ts_tag_continuation(&description)
        ));
    }

    if !tags.is_empty() {
        prose_blocks.push(tags.join("\n"));
    }
    if !documentation.examples.is_empty() {
        prose_blocks.push(format!(
            "@example\n{}",
            typescript_documentation_text(&documentation.examples, context)
        ));
    }

    emit_ts_doc_block(source, &prose_blocks.join("\n\n"), indent)
}

/// Emit one TSDoc block without ever collapsing its delimiters onto prose.
fn emit_ts_doc_block(source: &mut String, docs: &str, indent: &str) -> Result<()> {
    writeln!(source, "{indent}/**")?;
    let mut fenced = false;
    for line in docs.lines() {
        if line.is_empty() {
            writeln!(source, "{indent} *")?;
        } else {
            let escaped = line.replace("*/", "* /");
            let fence = escaped.trim_start().starts_with("```");
            let lines = if fenced || fence {
                vec![escaped]
            } else {
                wrap_ts_doc_line(&escaped, 97usize.saturating_sub(indent.len()))
            };
            for line in lines {
                writeln!(source, "{indent} * {line}")?;
            }
            if fence {
                fenced = !fenced;
            }
        }
    }
    writeln!(source, "{indent} */")?;
    Ok(())
}

/// Translate inline Rust callable links to the names exposed by TypeScript.
pub(crate) fn typescript_documentation_text(
    value: &str,
    context: &TypeScriptContext<'_>,
) -> String {
    let mut rendered = remove_rustdoc_link_brackets(value);
    for function in &context.manifest.functions {
        rendered = rendered.replace(
            &format!("`{}`", function.rust_name),
            &format!("`{}`", function.host_name),
        );
    }
    for resource in &context.manifest.resources {
        for callable in &resource.constructors {
            rendered = rendered.replace(
                &format!("`{}`", callable.rust_name),
                &format!("`{}`", callable.host_name),
            );
        }
        for method in &resource.methods {
            rendered = rendered.replace(
                &format!("`{}`", method.rust_name),
                &format!("`{}`", method.host_name),
            );
        }
    }
    rendered
}

/// Wrap one TSDoc line while keeping continuation text inside its block tag.
fn wrap_ts_doc_line(value: &str, width: usize) -> Vec<String> {
    let leading = value.len() - value.trim_start().len();
    let continuation = if value.trim_start().starts_with('@') {
        leading + 2
    } else {
        leading
    };
    let continuation_prefix = " ".repeat(continuation);
    let mut prefix = value[..leading].to_owned();
    let mut remaining = value.trim_start();
    let mut lines = Vec::new();

    while prefix.chars().count() + remaining.chars().count() > width {
        let available = width.saturating_sub(prefix.chars().count());
        let boundary = remaining
            .char_indices()
            .take_while(|(index, _)| remaining[..*index].chars().count() <= available)
            .filter(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index)
            .last();
        let Some(boundary) = boundary.filter(|boundary| *boundary > 0) else {
            break;
        };
        lines.push(format!("{prefix}{}", remaining[..boundary].trim_end()));
        remaining = remaining[boundary..].trim_start();
        prefix.clone_from(&continuation_prefix);
    }
    lines.push(format!("{prefix}{remaining}"));
    lines
}

/// Append one paragraph to a TSDoc remarks block.
fn append_ts_paragraph(target: &mut String, value: &str) {
    if !target.is_empty() {
        target.push_str("\n\n");
    }
    target.push_str(value);
}

/// Keep authored continuation lines visually inside their TSDoc block tag.
fn indent_ts_tag_continuation(value: &str) -> String {
    value.replace('\n', "\n  ")
}

/// Render an object property as an identifier or quoted key.
pub(crate) fn ts_property(value: &str) -> String {
    if is_identifier(value) {
        value.to_owned()
    } else {
        ts_string(value)
    }
}

/// Quote a JavaScript string literal with deterministic JSON escaping.
pub(crate) fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

/// Return the typed-array name for a contract buffer element.
pub(crate) fn typescript_buffer_name(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "Uint8Array",
        BufferElement::I8 => "Int8Array",
        BufferElement::U16 => "Uint16Array",
        BufferElement::I16 => "Int16Array",
        BufferElement::U32 => "Uint32Array",
        BufferElement::I32 => "Int32Array",
        BufferElement::U64 => "BigUint64Array",
        BufferElement::I64 => "BigInt64Array",
        BufferElement::F32 => "Float32Array",
        BufferElement::F64 => "Float64Array",
    }
}

/// Return the runtime schema key for a contract buffer element.
pub(crate) fn buffer_key(element: BufferElement) -> &'static str {
    match element {
        BufferElement::U8 => "u8",
        BufferElement::I8 => "i8",
        BufferElement::U16 => "u16",
        BufferElement::I16 => "i16",
        BufferElement::U32 => "u32",
        BufferElement::I32 => "i32",
        BufferElement::U64 => "u64",
        BufferElement::I64 => "i64",
        BufferElement::F32 => "f32",
        BufferElement::F64 => "f64",
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
            errors: Vec::new(),
            functions: Vec::new(),
            resources: Vec::new(),
            constants: Vec::new(),
        }
    }

    #[test]
    fn recursive_lists_use_generic_array_syntax() {
        let manifest = manifest();
        let namespace = Namespace::root();
        let context = TypeScriptContext {
            manifest: &manifest,
            namespace: &namespace,
        };

        assert_eq!(
            type_ref(
                &TypeRef::List {
                    item: Box::new(TypeRef::Option {
                        item: Box::new(TypeRef::String),
                    }),
                },
                &context,
            )
            .expect("render list"),
            "ReadonlyArray<string | null>"
        );
        assert_eq!(
            type_ref(
                &TypeRef::List {
                    item: Box::new(TypeRef::List {
                        item: Box::new(TypeRef::Float { bits: 64 }),
                    }),
                },
                &context,
            )
            .expect("render nested list"),
            "ReadonlyArray<ReadonlyArray<number>>"
        );
    }

    #[test]
    fn wide_json_integer_constants_render_as_bigints() {
        assert_eq!(
            typescript_json_value(&serde_json::json!(u64::MAX)).expect("render wide integer"),
            format!("{}n", u64::MAX)
        );
        assert_eq!(
            typescript_json_value(&serde_json::json!({"id": u64::MAX}))
                .expect("render nested wide integer"),
            format!("{{id: {}n}}", u64::MAX)
        );
    }

    #[test]
    fn ordinary_tsdoc_always_uses_multiline_delimiters() {
        let mut source = String::new();
        emit_ts_doc(&mut source, Some("Short documentation."), "  ").expect("render TSDoc");

        assert_eq!(source, "  /**\n   * Short documentation.\n   */\n");
        assert!(!source.contains("/** Short"));
    }

    #[test]
    fn callable_tsdoc_uses_standard_block_tags() {
        let mut manifest = manifest();
        manifest.errors.push(rspyts::ir::ErrorDef {
            owner: rspyts::ir::CargoPackageId::new("test"),
            rust_module: "test".to_owned(),
            id: "test::ExampleError".to_owned(),
            name: "ExampleError".to_owned(),
            docs: None,
        });
        let namespace = Namespace::root();
        let context = TypeScriptContext {
            manifest: &manifest,
            namespace: &namespace,
        };
        let params = vec![ParamDef {
            rust_name: "value".to_owned(),
            host_name: "value".to_owned(),
            ty: TypeRef::String,
        }];
        let error = rspyts::ir::DefinitionId::new("test", "test::ExampleError");
        let mut source = String::new();

        emit_ts_callable_doc(
            &mut source,
            &CallableDocumentation {
                docs: Some(
                    "Process a value.\n\nRetains ordering.\n\n# Arguments\n\n- `value` - Value to process.\n\n# Returns\n\nThe processed value.\n\n# Errors\n\nReturns `ExampleError` when validation fails.",
                ),
                fallback_summary: "Fallback.".to_owned(),
                params: &params,
                returns: CallableReturn::Contract(&TypeRef::String),
                error: Some(&error),
            },
            &context,
            "",
        )
        .expect("render callable TSDoc");

        assert!(source.starts_with("/**\n * Process a value.\n"));
        assert!(source.contains(" * @remarks\n * Retains ordering."));
        assert!(source.contains(" * @param value - Value to process."));
        assert!(source.contains(" * @returns The processed value."));
        assert!(source.contains(
            " * @throws {@link ExampleError} - The Rust implementation returns `ExampleError` when"
        ));
        assert!(!source.contains("# Errors"));
        assert!(!source.contains("\n * @param value - Value to process.\n *\n * @returns"));
    }

    #[test]
    fn typescript_docs_use_public_callable_names() {
        let mut manifest = manifest();
        manifest.functions.push(FunctionDef {
            owner: rspyts::ir::CargoPackageId::new("test"),
            rust_module: "test".to_owned(),
            rust_name: "load_value".to_owned(),
            host_name: "loadValue".to_owned(),
            native_name: "native_load_value".to_owned(),
            docs: None,
            params: Vec::new(),
            returns: TypeRef::Unit,
            error: None,
        });
        let namespace = Namespace::root();
        let context = TypeScriptContext {
            manifest: &manifest,
            namespace: &namespace,
        };

        assert_eq!(
            typescript_documentation_text("Call [`load_value`] next.", &context),
            "Call `loadValue` next."
        );
    }
}
