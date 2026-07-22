use super::*;

pub(super) fn python_models(
    items: &NamespaceItems<'_>,
    context: &PythonContext<'_>,
) -> Result<String> {
    let imports = model_imports(items);
    let buffers = namespace_buffers(items);
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
    for namespace in python_model_imports(items, context)? {
        writeln!(
            source,
            "\nimport {} as {}",
            python_module(context.package, &namespace, "models"),
            python_model_alias(&namespace, context)
        )?;
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

    for definition in &items.types {
        emit_python_type(&mut source, definition, context)?;
    }
    let mut rebuilds = Vec::new();
    for definition in &items.types {
        match definition.shape {
            TypeShape::Struct { .. } => {
                rebuilds.push(definition.name.clone());
            }
            TypeShape::TaggedEnum { ref variants, .. } => {
                for variant in variants {
                    rebuilds.push(tagged_variant_name(&definition.name, &variant.rust_name));
                }
            }
            TypeShape::StringEnum { .. } | TypeShape::Alias { .. } => {}
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

fn model_imports(items: &NamespaceItems<'_>) -> ModelImports {
    let mut imports = ModelImports::default();
    if !namespace_buffers(items).is_empty() {
        imports.typing.extend(["Annotated", "TypeAlias"]);
    }
    for definition in &items.types {
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
                imports.typing.insert("TypeAlias");
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

pub(super) fn begin_python_top_level(source: &mut String) {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    while !source.ends_with("\n\n\n") {
        source.push('\n');
    }
}

pub(super) fn begin_python_alias(source: &mut String) {
    if !source.ends_with('\n') {
        source.push('\n');
    }
    while !source.ends_with("\n\n") {
        source.push('\n');
    }
}

fn emit_python_type(
    source: &mut String,
    definition: &TypeDef,
    context: &PythonContext<'_>,
) -> Result<()> {
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
                emit_python_field(source, field, context, "    ")?;
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
                    emit_python_field(source, field, context, "    ")?;
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
            begin_python_alias(source);
            writeln!(
                source,
                "{}: TypeAlias = {}",
                definition.name,
                python_ref(target, context)?
            )?;
        }
    }
    Ok(())
}

fn emit_model_config(source: &mut String) {
    source.push_str(
        "    model_config = ConfigDict(\n        frozen=True,\n        populate_by_name=True,\n        extra=\"forbid\",\n        arbitrary_types_allowed=True,\n    )\n",
    );
}

fn emit_python_field(
    source: &mut String,
    field: &FieldDef,
    context: &PythonContext<'_>,
    indent: &str,
) -> Result<()> {
    if let Some(docs) = field.docs.as_deref() {
        writeln!(source, "{indent}# {}", docs.replace('\n', " "))?;
    }
    let annotation = if let Some(literal) = &field.constraints.literal {
        format!("Literal[{}]", python_scalar(literal))
    } else {
        python_ref(&field.ty, context)?
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
