use super::*;

pub(super) fn typescript_declarations(
    items: &NamespaceItems<'_>,
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    let mut source = String::new();
    for import in typescript_type_imports(items, context)? {
        writeln!(
            source,
            "import type * as {} from {};",
            namespace_alias(&import),
            ts_string(&typescript_import(context.package, &import))
        )?;
    }
    if namespace_refs(items)
        .iter()
        .any(|reference| reference_contains(reference, &|item| matches!(item, TypeRef::Json)))
    {
        source.push_str("\nexport type JsonValue = null | boolean | number | string | JsonValue[] | { readonly [key: string]: JsonValue };\n");
    }
    for definition in &items.types {
        emit_typescript_type(&mut source, definition, context)?;
    }
    for error in &items.errors {
        writeln!(
            source,
            "\nexport class {} extends globalThis.Error {{\n  readonly code: string;\n  constructor(code: string, message: string);\n}}",
            error.name
        )?;
    }
    for function in &items.functions {
        writeln!(
            source,
            "\nexport function {}({}): {};",
            function.host_name,
            typescript_params(&function.params, context)?,
            return_type_ref(&function.returns, context)?
        )?;
    }
    for resource in &items.resources {
        emit_typescript_resource_declaration(&mut source, resource, context)?;
    }
    for constant in &items.constants {
        writeln!(
            source,
            "\nexport const {}: {};",
            constant.host_name,
            type_ref(&constant.ty, context)?
        )?;
    }
    Ok(source)
}

fn emit_typescript_type(
    source: &mut String,
    definition: &TypeDef,
    context: &TypeScriptContext<'_>,
) -> Result<()> {
    match &definition.shape {
        TypeShape::Struct { fields } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            writeln!(source, "export interface {} {{", definition.name)?;
            for field in fields {
                emit_ts_doc(source, field.docs.as_deref(), "  ")?;
                writeln!(
                    source,
                    "  readonly {}{}: {};",
                    ts_property(&field.wire_name),
                    if field.required { "" } else { "?" },
                    type_ref(&field.ty, context)?
                )?;
            }
            source.push_str("}\n");
        }
        TypeShape::StringEnum { variants } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            let union = variants
                .iter()
                .map(|variant| ts_string(&variant.wire_name))
                .collect::<Vec<_>>()
                .join(" | ");
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                if union.is_empty() { "never" } else { &union }
            )?;
            writeln!(source, "export declare const {}: {{", definition.name)?;
            for variant in variants {
                writeln!(
                    source,
                    "  readonly {}: {};",
                    ts_property(&variant.rust_name),
                    ts_string(&variant.wire_name),
                )?;
            }
            source.push_str("};\n");
        }
        TypeShape::TaggedEnum { tag, variants } => {
            for variant in variants {
                let name = tagged_variant_name(&definition.name, &variant.rust_name);
                emit_ts_doc(source, variant.docs.as_deref(), "")?;
                writeln!(source, "export interface {name} {{")?;
                writeln!(
                    source,
                    "  readonly {}: {};",
                    ts_property(tag),
                    ts_string(&variant.wire_name)
                )?;
                for field in &variant.fields {
                    emit_ts_doc(source, field.docs.as_deref(), "  ")?;
                    writeln!(
                        source,
                        "  readonly {}{}: {};",
                        ts_property(&field.wire_name),
                        if field.required { "" } else { "?" },
                        type_ref(&field.ty, context)?
                    )?;
                }
                source.push_str("}\n");
            }
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                variants
                    .iter()
                    .map(|variant| tagged_variant_name(&definition.name, &variant.rust_name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )?;
        }
        TypeShape::Alias { target } => {
            emit_ts_doc(source, definition.docs.as_deref(), "")?;
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                type_ref(target, context)?
            )?;
        }
    }
    Ok(())
}

fn emit_typescript_resource_declaration(
    source: &mut String,
    resource: &ResourceDef,
    context: &TypeScriptContext<'_>,
) -> Result<()> {
    let constructor = resource
        .constructors
        .iter()
        .find(|item| item.rust_name == "new")
        .or_else(|| resource.constructors.first())
        .context("resource has no constructor")?;
    emit_ts_doc(source, resource.docs.as_deref(), "")?;
    writeln!(source, "export class {} {{", resource.name)?;
    writeln!(
        source,
        "  constructor({});",
        typescript_params(&constructor.params, context)?
    )?;
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        writeln!(
            source,
            "  static {}({}): {};",
            factory.host_name,
            typescript_params(&factory.params, context)?,
            resource.name
        )?;
    }
    for method in &resource.methods {
        writeln!(
            source,
            "  {}({}): {};",
            method.host_name,
            typescript_params(&method.params, context)?,
            return_type_ref(&method.returns, context)?
        )?;
    }
    source.push_str("  close(): void;\n}\n");
    Ok(())
}
