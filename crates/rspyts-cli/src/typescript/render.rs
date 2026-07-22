use super::*;

pub(crate) fn type_ref(reference: &TypeRef, context: &TypeScriptContext<'_>) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "null".into(),
        TypeRef::Bool => "boolean".into(),
        TypeRef::Int { bits: 64, .. } => "bigint".into(),
        TypeRef::Int { .. } | TypeRef::Float { .. } => "number".into(),
        TypeRef::String | TypeRef::DateTime => "string".into(),
        TypeRef::Json => "JsonValue".into(),
        TypeRef::Option { item } => format!("{} | null", type_ref(item, context)?),
        TypeRef::List { item } => format!("readonly {}[]", type_ref(item, context)?),
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

pub(crate) fn return_type_ref(
    reference: &TypeRef,
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    if matches!(reference, TypeRef::Unit) {
        return Ok("void".into());
    }
    type_ref(reference, context)
}

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

pub(crate) fn api_namespace_alias(namespace: &Namespace) -> String {
    format!("api_{}", &namespace_alias(namespace)["types_".len()..])
}

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

pub(crate) fn typescript_runtime_path(namespace: &Namespace) -> String {
    let depth = namespace.typescript_segments().len();
    if depth == 0 {
        "./runtime.js".to_owned()
    } else {
        format!("{}runtime.js", "../".repeat(depth))
    }
}

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
        _ => serde_json::to_string(value)?,
    })
}

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

pub(crate) fn emit_ts_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs {
        writeln!(
            source,
            "{indent}/** {} */",
            docs.replace("*/", "* /").replace('\n', " ")
        )?;
    }
    Ok(())
}

pub(crate) fn ts_property(value: &str) -> String {
    if is_identifier(value) {
        value.to_owned()
    } else {
        ts_string(value)
    }
}

pub(crate) fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

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
