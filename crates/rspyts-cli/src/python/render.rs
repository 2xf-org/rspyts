use super::*;

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

pub(crate) fn python_param(param: &ParamDef, context: &PythonContext<'_>) -> Result<String> {
    Ok(format!(
        "{}: {}",
        safe_python_name(&param.rust_name),
        python_ref(&param.ty, context)?
    ))
}

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

pub(crate) fn namespace_buffers(items: &NamespaceItems<'_>) -> BTreeSet<BufferElement> {
    let mut buffers = BTreeSet::new();
    for reference in namespace_refs(items) {
        collect_buffers(reference, &mut buffers);
    }
    buffers
}

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

pub(crate) fn python_model_alias(namespace: &Namespace, context: &PythonContext<'_>) -> String {
    let index = namespaces(context.manifest)
        .keys()
        .position(|candidate| candidate == namespace)
        .expect("referenced namespace belongs to the manifest");
    format!("_rspyts_models_{index}")
}

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

pub(crate) fn python_module(package: &str, namespace: &Namespace, leaf: &str) -> String {
    package
        .split('.')
        .map(str::to_owned)
        .chain(namespace.python_segments())
        .chain([leaf.to_owned()])
        .collect::<Vec<_>>()
        .join(".")
}

pub(crate) fn python_scalar(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(value) => if *value { "True" } else { "False" }.into(),
        ScalarValue::I64(value) => value.to_string(),
        ScalarValue::String(value) => py_string(value),
    }
}

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

pub(crate) fn emit_python_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
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

pub(crate) fn wrap_python_doc(value: &str, width: usize) -> Vec<String> {
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

pub(crate) fn safe_python_name(value: &str) -> String {
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

pub(crate) fn py_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

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
