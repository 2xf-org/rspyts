use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rspyts::ir::{
    BufferElement, FieldConstraints, FieldDef, FunctionDef, Manifest, Namespace, ParamDef,
    ResourceDef, TypeDef, TypeRef, TypeShape,
};
use serde_json::{Value, json};
use wasm_bindgen_cli_support::Bindgen;

use crate::contract::{
    NamespaceItems, definition_key, error_definition, named_identities, namespace_refs, namespaces,
    tagged_variant_name, type_definition, type_namespace,
};
use crate::output::{write, write_json};
use crate::project::{Project, is_identifier};

struct TypeScriptContext<'a> {
    manifest: &'a Manifest,
    package: &'a str,
    namespace: &'a Namespace,
}

pub(super) fn emit(project: &Project, manifest: &Manifest, wasm: &Path, root: &Path) -> Result<()> {
    let package = root.join("typescript");
    fs::create_dir_all(&package)?;
    let mut bindgen = Bindgen::new();
    bindgen
        .input_path(wasm)
        .web(true)?
        .typescript(false)
        .omit_default_module_path(false)
        .out_name("native")
        .generate(&package)
        .context("failed to generate TypeScript WebAssembly bindings")?;
    write(&package.join("runtime.js"), &typescript_runtime(manifest)?)?;
    let namespace_map = namespaces(manifest);
    for (namespace, items) in &namespace_map {
        let namespace_package = namespace
            .typescript_segments()
            .iter()
            .fold(package.clone(), |path, segment| path.join(segment));
        fs::create_dir_all(&namespace_package)?;
        let context = TypeScriptContext {
            manifest,
            package: &project.typescript_package,
            namespace,
        };
        write(
            &namespace_package.join("index.d.ts"),
            &typescript_declarations(items, &context)?,
        )?;
        write(
            &namespace_package.join("index.js"),
            &typescript_api(items, &context)?,
        )?;
    }
    let package_json = package_manifest(
        &project.typescript_package,
        &manifest.package_version,
        namespace_map.keys(),
    );
    write_json(&package.join("package.json"), &package_json)
}

fn package_manifest<'a>(
    package_name: &str,
    package_version: &str,
    namespaces: impl Iterator<Item = &'a Namespace>,
) -> Value {
    let mut exports = serde_json::Map::new();
    for namespace in namespaces {
        let segments = namespace.typescript_segments();
        let subpath = if segments.is_empty() {
            ".".to_owned()
        } else {
            format!("./{}", segments.join("/"))
        };
        let file = if segments.is_empty() {
            "./index".to_owned()
        } else {
            format!("./{}/index", segments.join("/"))
        };
        exports.insert(
            subpath,
            json!({
                "types": format!("{file}.d.ts"),
                "import": format!("{file}.js")
            }),
        );
    }
    json!({
        "name": package_name,
        "version": package_version,
        "type": "module",
        "sideEffects": true,
        "types": "./index.d.ts",
        "exports": exports,
        "files": ["**/*.js", "**/*.d.ts", "native_bg.wasm"]
    })
}

fn typescript_declarations(
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
            "\nexport class {} extends Error {{\n  readonly code: string;\n  constructor(code: string, message: string);\n}}",
            error.name
        )?;
    }
    for function in &items.functions {
        writeln!(
            source,
            "\nexport function {}({}): {};",
            function.host_name,
            typescript_params(&function.params, context)?,
            type_ref(&function.returns, context)?
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
            writeln!(
                source,
                "export type {} = {};",
                definition.name,
                variants
                    .iter()
                    .map(|variant| ts_string(&variant.wire_name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )?;
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
            type_ref(&method.returns, context)?
        )?;
    }
    source.push_str("  close(): void;\n}\n");
    Ok(())
}

fn typescript_api(items: &NamespaceItems<'_>, context: &TypeScriptContext<'_>) -> Result<String> {
    let runtime_imports = typescript_runtime_imports(items);
    let mut source = String::new();
    if !runtime_imports.is_empty() {
        source.push_str("import {\n");
        for name in runtime_imports {
            writeln!(source, "  {name},")?;
        }
        writeln!(
            source,
            "}} from {};",
            ts_string(&typescript_runtime_path(context.namespace))
        )?;
    }
    for import in typescript_error_imports(items, context)? {
        writeln!(
            source,
            "import * as {} from {};",
            namespace_alias(&import),
            ts_string(&typescript_import(context.package, &import))
        )?;
    }
    for error in &items.errors {
        writeln!(
            source,
            "\nexport class {} extends Error {{\n  constructor(code, message) {{\n    super(message);\n    this.name = {};\n    this.code = code;\n  }}\n}}",
            error.name,
            ts_string(&error.name)
        )?;
    }
    for function in &items.functions {
        emit_typescript_function(&mut source, function, context, None)?;
    }
    for resource in &items.resources {
        emit_typescript_resource(&mut source, resource, context)?;
    }
    for constant in &items.constants {
        writeln!(
            source,
            "\nexport const {} = restoreHost({}, {});",
            constant.host_name,
            typescript_value(&constant.value, &constant.ty, context.manifest)?,
            typescript_spec(&constant.ty)?
        )?;
    }
    Ok(source)
}

fn typescript_runtime(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "import initializeNative, * as native from \"./native.js\";\n\nconst wasmUrl = new URL(\"./native_bg.wasm\", import.meta.url);\nlet wasmInput = wasmUrl;\nif (wasmUrl.protocol === \"file:\" && globalThis.process?.versions?.node) {\n  const nodeModule = \"node:fs/promises\";\n  const { readFile } = await import(nodeModule);\n  wasmInput = await readFile(wasmUrl);\n}\nawait initializeNative({ module_or_path: wasmInput });\n\nexport { native };\n",
    );
    source.push_str(TYPESCRIPT_ADAPTERS);
    source.push_str("\nconst nativeSchemas = {\n");
    for definition in &manifest.types {
        writeln!(
            source,
            "  {}: {},",
            ts_property(&definition_key(&definition.identity())),
            typescript_named_spec(definition)?
        )?;
    }
    source.push_str("};\n");
    Ok(source)
}

fn typescript_runtime_imports(items: &NamespaceItems<'_>) -> Vec<&'static str> {
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
    let restores_values = !items.functions.is_empty()
        || items
            .resources
            .iter()
            .any(|resource| !resource.methods.is_empty())
        || !items.constants.is_empty();
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
        imports.push("nativeError");
    }
    if has_params {
        imports.push("prepareHost");
    }
    if restores_values {
        imports.push("restoreHost");
    }
    imports
}

const TYPESCRIPT_ADAPTERS: &str = r#"
export function prepareHost(value) {
  if (value instanceof Date) return value.toISOString();
  if (ArrayBuffer.isView(value)) return value;
  if (Array.isArray(value)) return value.map(prepareHost);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item)]));
  }
  return value;
}

const bufferConstructors = {
  u8: Uint8Array, i8: Int8Array, u16: Uint16Array, i16: Int16Array,
  u32: Uint32Array, i32: Int32Array, u64: BigUint64Array, i64: BigInt64Array,
  f32: Float32Array, f64: Float64Array,
};

export function restoreHost(value, spec) {
  if (value == null || spec == null) return value;
  const [kind, detail, variants] = spec;
  if (kind === "bytes") return new Uint8Array(value);
  if (kind === "buffer") return new bufferConstructors[detail](value);
  if (kind === "list") return Array.from(value, item => restoreHost(item, detail));
  if (kind === "map") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail)]));
  if (kind === "tuple") return value.map((item, index) => restoreHost(item, detail[index]));
  if (kind === "named") return restoreHost(value, nativeSchemas[detail]);
  if (kind === "alias") return restoreHost(value, detail);
  if (kind === "struct") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail[key])]));
  if (kind === "tagged") {
    const fields = variants[value[detail]] ?? {};
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, fields[key])]));
  }
  return value;
}

export function nativeError(error, ErrorType) {
  const text = String(error);
  const line = text.indexOf("\n");
  return line < 0 ? error : new ErrorType(text.slice(0, line), text.slice(line + 1));
}
"#;

fn emit_typescript_function(
    source: &mut String,
    function: &FunctionDef,
    context: &TypeScriptContext<'_>,
    receiver: Option<&str>,
) -> Result<()> {
    let params = function
        .params
        .iter()
        .map(|param| param.host_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let calls = function
        .params
        .iter()
        .map(|param| format!("prepareHost({})", param.host_name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut result_name = "nativeResult".to_owned();
    let parameter_names = function
        .params
        .iter()
        .map(|param| param.host_name.as_str())
        .collect::<BTreeSet<_>>();
    while parameter_names.contains(result_name.as_str()) {
        result_name.push_str("Value");
    }
    let native_name = format!("__rspyts_export_{}", function.host_name);
    let (indent, signature, call) = if receiver.is_some() {
        (
            "  ",
            format!("  {}({params}) {{", function.host_name),
            format!("this.nativeResource.{}({calls})", function.host_name),
        )
    } else {
        (
            "",
            format!("export function {}({params}) {{", function.host_name),
            format!(
                "native[{quoted}]({calls})",
                quoted = ts_string(&native_name)
            ),
        )
    };
    writeln!(source, "\n{signature}")?;
    if function.error.is_some() {
        writeln!(source, "{indent}  try {{")?;
        writeln!(source, "{indent}    const {result_name} = {call};")?;
        writeln!(
            source,
            "{indent}    return restoreHost({result_name}, {});",
            typescript_spec(&function.returns)?
        )?;
        writeln!(source, "{indent}  }} catch (error) {{")?;
        writeln!(
            source,
            "{indent}    throw nativeError(error, {});",
            typescript_error_ref(function.error.as_ref(), context)?
        )?;
        writeln!(source, "{indent}  }}")?;
    } else {
        writeln!(source, "{indent}  const {result_name} = {call};")?;
        writeln!(
            source,
            "{indent}  return restoreHost({result_name}, {});",
            typescript_spec(&function.returns)?
        )?;
    }
    writeln!(source, "{indent}}}")?;
    Ok(())
}

fn emit_typescript_resource(
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
    let params = constructor
        .params
        .iter()
        .map(|item| item.host_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let calls = constructor
        .params
        .iter()
        .map(|item| format!("prepareHost({})", item.host_name))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(source, "\nexport class {} {{", resource.name)?;
    writeln!(source, "  constructor({params}) {{")?;
    let native_call = format!("new native.RspytsWasm{}({calls})", resource.name);
    if constructor.error.is_some() {
        source.push_str("    try {\n");
        writeln!(source, "      this.nativeResource = {native_call};")?;
        source.push_str("    } catch (error) {\n");
        writeln!(
            source,
            "      throw nativeError(error, {});",
            typescript_error_ref(constructor.error.as_ref(), context)?
        )?;
        source.push_str("    }\n");
    } else {
        writeln!(source, "    this.nativeResource = {native_call};")?;
    }
    source.push_str("  }\n");
    for factory in resource
        .constructors
        .iter()
        .filter(|item| !std::ptr::eq(*item, constructor))
    {
        let params = factory
            .params
            .iter()
            .map(|item| item.host_name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let calls = factory
            .params
            .iter()
            .map(|item| format!("prepareHost({})", item.host_name))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(source, "\n  static {}({params}) {{", factory.host_name)?;
        writeln!(
            source,
            "    const value = Object.create({}.prototype);",
            resource.name
        )?;
        let native_call = format!(
            "native.RspytsWasm{}.{}({calls})",
            resource.name, factory.host_name
        );
        if factory.error.is_some() {
            source.push_str("    try {\n");
            writeln!(source, "      value.nativeResource = {native_call};")?;
            source.push_str("    } catch (error) {\n");
            writeln!(
                source,
                "      throw nativeError(error, {});",
                typescript_error_ref(factory.error.as_ref(), context)?
            )?;
            source.push_str("    }\n");
        } else {
            writeln!(source, "    value.nativeResource = {native_call};")?;
        }
        source.push_str("    return value;\n  }\n");
    }
    for method in &resource.methods {
        let function = FunctionDef {
            owner: resource.owner.clone(),
            rust_module: resource.rust_module.clone(),
            rust_name: method.rust_name.clone(),
            host_name: method.host_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_typescript_function(source, &function, context, Some(&resource.name))?;
    }
    source.push_str("\n  close() {\n    this.nativeResource.close();\n  }\n}\n");
    Ok(())
}

fn type_ref(reference: &TypeRef, context: &TypeScriptContext<'_>) -> Result<String> {
    Ok(match reference {
        TypeRef::Unit => "void".into(),
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

#[cfg(test)]
pub(super) fn test_type_ref(reference: &TypeRef, manifest: &Manifest) -> Result<String> {
    let namespace = Namespace::root();
    type_ref(
        reference,
        &TypeScriptContext {
            manifest,
            package: &manifest.package_name,
            namespace: &namespace,
        },
    )
}

fn typescript_type_imports(
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

fn typescript_error_imports(
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

fn typescript_named_ref(
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

fn typescript_error_ref(
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
            namespace_alias(&namespace),
            definition.name
        ))
    }
}

fn namespace_alias(namespace: &Namespace) -> String {
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

fn typescript_import(package: &str, namespace: &Namespace) -> String {
    let segments = namespace.typescript_segments();
    if segments.is_empty() {
        package.to_owned()
    } else {
        format!("{package}/{}", segments.join("/"))
    }
}

fn typescript_runtime_path(namespace: &Namespace) -> String {
    let depth = namespace.typescript_segments().len();
    if depth == 0 {
        "./runtime.js".to_owned()
    } else {
        format!("{}runtime.js", "../".repeat(depth))
    }
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

fn typescript_params(params: &[ParamDef], context: &TypeScriptContext<'_>) -> Result<String> {
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

fn typescript_spec(reference: &TypeRef) -> Result<String> {
    Ok(match reference {
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

fn typescript_named_spec(definition: &TypeDef) -> Result<String> {
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

fn typescript_value(value: &Value, reference: &TypeRef, manifest: &Manifest) -> Result<String> {
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

fn typescript_named_value(
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

fn typescript_object_value(
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

fn emit_ts_doc(source: &mut String, docs: Option<&str>, indent: &str) -> Result<()> {
    if let Some(docs) = docs {
        writeln!(
            source,
            "{indent}/** {} */",
            docs.replace("*/", "* /").replace('\n', " ")
        )?;
    }
    Ok(())
}

fn ts_property(value: &str) -> String {
    if is_identifier(value) {
        value.to_owned()
    } else {
        ts_string(value)
    }
}

fn ts_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

fn typescript_buffer_name(element: BufferElement) -> &'static str {
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

fn buffer_key(element: BufferElement) -> &'static str {
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
    use rspyts::ir::{
        CargoPackageId, ConstantDef, EnumVariantDef, ErrorDef, FieldConstraints, FieldDef,
        FunctionDef, Manifest, Namespace, ParamDef, TypeDef, TypeRef, TypeShape,
    };
    use serde_json::json;

    use super::{
        TypeScriptContext, package_manifest, typescript_api, typescript_declarations,
        typescript_runtime,
    };
    use crate::contract::namespaces;

    #[test]
    fn generated_package_has_unambiguous_entry_points() {
        let namespace = Namespace::root();
        let package = package_manifest("example", "1.0.0", [&namespace].into_iter());

        assert_eq!(package["types"], "./index.d.ts");
        assert_eq!(package["exports"]["."]["import"], "./index.js");
        assert!(
            !package["files"]
                .as_array()
                .expect("files are an array")
                .iter()
                .any(|file| file == "native.d.ts")
        );
        assert!(
            package["files"]
                .as_array()
                .expect("files are an array")
                .iter()
                .any(|file| file == "**/*.js")
        );
    }

    #[test]
    fn generated_package_exports_empty_parent_namespaces() {
        let manifest = Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            package_name: "example".to_owned(),
            package_version: "1.2.3".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: vec![FunctionDef {
                owner: CargoPackageId::new("example-dice"),
                rust_module: "example_dice::fair::deep::roll".to_owned(),
                rust_name: "roll".to_owned(),
                host_name: "roll".to_owned(),
                docs: None,
                params: Vec::new(),
                returns: TypeRef::Unit,
                error: None,
            }],
            resources: Vec::new(),
            constants: Vec::new(),
        };
        let views = namespaces(&manifest);
        let package = package_manifest("example", "1.2.3", views.keys());

        for subpath in [
            ".",
            "./dice",
            "./dice/fair",
            "./dice/fair/deep",
            "./dice/fair/deep/roll",
        ] {
            assert!(
                package["exports"].get(subpath).is_some(),
                "missing {subpath}"
            );
        }
        let parent = Namespace {
            package: Some("dice".to_owned()),
            modules: vec!["fair".to_owned()],
        };
        let context = TypeScriptContext {
            manifest: &manifest,
            package: "example",
            namespace: &parent,
        };
        assert!(
            typescript_declarations(&views[&parent], &context)
                .expect("parent declarations generate")
                .is_empty()
        );
        assert!(
            typescript_api(&views[&parent], &context)
                .expect("parent API generates")
                .is_empty()
        );
    }

    #[test]
    fn generated_api_keeps_boundary_code_in_the_runtime_module() {
        let mut manifest = Manifest {
            ir_version: 2,
            package_name: "example".into(),
            package_version: "1.0.0".into(),
            module_name: "native".into(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: vec![FunctionDef {
                owner: CargoPackageId::new("example"),
                rust_module: "example".into(),
                rust_name: "ping".into(),
                host_name: "ping".into(),
                docs: None,
                params: Vec::new(),
                returns: TypeRef::Unit,
                error: None,
            }],
            resources: Vec::new(),
            constants: Vec::new(),
        };

        let namespace = Namespace::root();
        let views = namespaces(&manifest);
        let api = typescript_api(
            views.get(&namespace).expect("root namespace"),
            &TypeScriptContext {
                manifest: &manifest,
                package: "example",
                namespace: &namespace,
            },
        )
        .expect("API generates");
        assert!(api.contains("from \"./runtime.js\""));
        assert!(!api.contains("initializeNative"));
        assert!(!api.contains("function prepareHost"));

        drop(views);
        manifest.functions[0].params.push(ParamDef {
            rust_name: "native_result".to_owned(),
            host_name: "nativeResult".to_owned(),
            ty: TypeRef::String,
        });
        manifest.functions[0].returns = TypeRef::String;
        let views = namespaces(&manifest);
        let api = typescript_api(
            views.get(&namespace).expect("root namespace"),
            &TypeScriptContext {
                manifest: &manifest,
                package: "example",
                namespace: &namespace,
            },
        )
        .expect("API generates");
        assert!(api.contains("const nativeResultValue ="));
        assert!(api.contains("restoreHost(nativeResultValue"));

        let runtime = typescript_runtime(&manifest).expect("runtime generates");
        assert!(runtime.contains("initializeNative"));
        assert!(runtime.contains("export function prepareHost"));
    }

    #[test]
    fn generated_typescript_resolves_all_cross_namespace_references() {
        let manifest = cross_namespace_manifest();
        let namespace = Namespace {
            package: Some("one".to_owned()),
            modules: vec!["service".to_owned()],
        };
        let views = namespaces(&manifest);
        let items = views.get(&namespace).expect("service namespace");
        let context = TypeScriptContext {
            manifest: &manifest,
            package: "example",
            namespace: &namespace,
        };

        let declarations = typescript_declarations(items, &context).expect("declarations generate");
        let api = typescript_api(items, &context).expect("API generates");
        let runtime = typescript_runtime(&manifest).expect("runtime generates");

        assert!(declarations.contains("from \"example/two/model\""));
        assert!(declarations.contains("target: types_two_model.Target"));
        assert!(
            declarations.contains("export function find(target: types_two_model.Target): Event")
        );
        assert!(declarations.contains("export const TARGET: types_two_model.Target"));
        assert!(api.contains("import * as types_two_model from \"example/two/model\""));
        assert!(api.contains("types_two_model.TargetError"));
        assert!(runtime.contains("example-two::example_two::model::Target"));
        assert!(runtime.contains("example-one::example_one::service::Event"));
    }

    fn cross_namespace_manifest() -> Manifest {
        let target = TypeDef {
            owner: CargoPackageId::new("example-two"),
            rust_module: "example_two::model".to_owned(),
            id: "example_two::model::Target".to_owned(),
            name: "Target".to_owned(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "value".to_owned(),
                    wire_name: "value".to_owned(),
                    docs: None,
                    ty: TypeRef::String,
                    required: true,
                    default: None,
                    constraints: FieldConstraints::default(),
                }],
            },
        };
        let target_ref = TypeRef::Named {
            identity: target.identity(),
        };
        let event = TypeDef {
            owner: CargoPackageId::new("example-one"),
            rust_module: "example_one::service".to_owned(),
            id: "example_one::service::Event".to_owned(),
            name: "Event".to_owned(),
            docs: None,
            shape: TypeShape::TaggedEnum {
                tag: "kind".to_owned(),
                variants: vec![EnumVariantDef {
                    rust_name: "Found".to_owned(),
                    wire_name: "found".to_owned(),
                    docs: None,
                    fields: vec![FieldDef {
                        rust_name: "target".to_owned(),
                        wire_name: "target".to_owned(),
                        docs: None,
                        ty: target_ref.clone(),
                        required: true,
                        default: None,
                        constraints: FieldConstraints::default(),
                    }],
                }],
            },
        };
        let error = ErrorDef {
            owner: CargoPackageId::new("example-two"),
            rust_module: "example_two::model".to_owned(),
            id: "example_two::model::TargetError".to_owned(),
            name: "TargetError".to_owned(),
            docs: None,
        };
        Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            package_name: "example".to_owned(),
            package_version: "1.2.3".to_owned(),
            module_name: "native".to_owned(),
            types: vec![event.clone(), target],
            errors: vec![error.clone()],
            functions: vec![FunctionDef {
                owner: CargoPackageId::new("example-one"),
                rust_module: "example_one::service".to_owned(),
                rust_name: "find".to_owned(),
                host_name: "find".to_owned(),
                docs: None,
                params: vec![ParamDef {
                    rust_name: "target".to_owned(),
                    host_name: "target".to_owned(),
                    ty: target_ref.clone(),
                }],
                returns: TypeRef::Named {
                    identity: event.identity(),
                },
                error: Some(error.identity()),
            }],
            resources: Vec::new(),
            constants: vec![ConstantDef {
                owner: CargoPackageId::new("example-one"),
                rust_module: "example_one::service".to_owned(),
                host_name: "TARGET".to_owned(),
                docs: None,
                ty: target_ref,
                value: json!({"value": "default"}),
            }],
        }
    }
}
