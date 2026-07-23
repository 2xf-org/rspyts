//! Rendering of TypeScript callables, resources, constants, and Wasm runtime.
//!
//! Public wrappers preserve strict host types while the runtime recursively
//! prepares Serde-shaped values and restores immutable results. Typed arrays
//! remain on wasm-bindgen's direct vector ABI.

use super::*;
use crate::documentation::{CallableDocumentation, CallableReturn};

/// Render errors, functions, resources, and constants for one namespace.
pub(super) fn typescript_api(
    items: &NamespaceItems<'_>,
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    let runtime_imports = typescript_runtime_imports(items);
    let mut source = String::new();
    let local_types = typescript_local_type_names(items);
    if !local_types.is_empty() {
        writeln!(
            source,
            "import type {{ {} }} from \"./models.js\";",
            local_types.join(", ")
        )?;
    }
    for import in typescript_type_imports(items, context)? {
        writeln!(
            source,
            "import type * as {} from {};",
            namespace_alias(&import),
            ts_string(&typescript_namespace_path(
                context.namespace,
                &import,
                "models.js"
            ))
        )?;
    }
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
            api_namespace_alias(&import),
            ts_string(&typescript_namespace_path(
                context.namespace,
                &import,
                "api.js"
            ))
        )?;
    }
    for error in &items.errors {
        source.push('\n');
        emit_ts_doc(
            &mut source,
            Some(
                error
                    .docs
                    .as_deref()
                    .unwrap_or("An error reported by the Rust implementation."),
            ),
            "",
        )?;
        writeln!(
            source,
            "export class {} extends globalThis.Error {{\n  /**\n   * Stable machine-readable error code.\n   */\n  readonly code: string;\n\n  constructor(code: string, message: string) {{\n    super(message);\n    this.name = {};\n    this.code = code;\n  }}\n}}",
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
        source.push('\n');
        let docs = constant
            .docs
            .as_deref()
            .map(|docs| typescript_documentation_text(docs, context));
        emit_ts_doc(&mut source, docs.as_deref(), "")?;
        writeln!(
            source,
            "export const {}: {} = restoreHost({}, {});",
            constant.host_name,
            type_ref(&constant.ty, context)?,
            typescript_value(&constant.value, &constant.ty, context.manifest)?,
            typescript_spec(&constant.ty)?
        )?;
    }
    Ok(source)
}

/// Render the package-level Wasm loader and recursive host adapters.
pub(super) fn typescript_runtime(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "import initializeNative, * as native from \"./native/native.js\";\n\nconst wasmUrl = new URL(\"./native/native_bg.wasm\", import.meta.url);\nlet wasmInput: any = wasmUrl;\nconst process = (globalThis as { process?: { versions?: { node?: string } } }).process;\nif (process?.versions?.node) {\n  const nodeModule = \"node:fs/promises\";\n  const { readFile } = await import(/* @vite-ignore */ nodeModule);\n  if (wasmUrl.protocol === \"file:\") {\n    wasmInput = await readFile(wasmUrl);\n  } else if (wasmUrl.pathname.startsWith(\"/@fs/\")) {\n    wasmInput = await readFile(decodeURIComponent(wasmUrl.pathname.slice(4)));\n  }\n}\nawait initializeNative({ module_or_path: wasmInput });\n\nexport { native };\n",
    );
    source.push_str(TYPESCRIPT_ADAPTERS);
    source.push_str("\nconst nativeSchemas: Record<string, any> = {\n");
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

// Import planning ----------------------------------------------------------

/// Return model names declared locally and needed by the API module.
fn typescript_local_type_names(items: &NamespaceItems<'_>) -> Vec<String> {
    let mut names = items
        .types
        .iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    if namespace_refs(items)
        .iter()
        .any(|reference| reference_contains(reference, &|item| matches!(item, TypeRef::Json)))
    {
        names.push("JsonValue".to_owned());
    }
    names.sort();
    names.dedup();
    names
}

/// Compute the runtime helpers required by one generated API module.
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
function prepareJson(value: any): any {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new RangeError("JSON numbers must be finite");
    if (Number.isInteger(value) && !Number.isSafeInteger(value)) {
      throw new RangeError("JSON integers outside JavaScript's safe range must be provided as bigint");
    }
    return value;
  }
  if (typeof value === "bigint" || value == null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(prepareJson);
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareJson(item)]));
}

export function prepareHost(value: any, spec: any = null): any {
  if (value == null) return value;
  if (value instanceof Date) return value.toISOString();
  if (ArrayBuffer.isView(value)) return value;
  if (spec != null) {
    const [kind, detail, variants] = spec;
    if (kind === "json") return prepareJson(value);
    if (kind === "named") return prepareHost(value, nativeSchemas[detail]);
    if (kind === "alias") return prepareHost(value, detail);
    if (kind === "list") return Array.from(value, (item: any) => prepareHost(item, detail));
    if (kind === "map") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item, detail)]));
    if (kind === "tuple") return value.map((item: any, index: number) => prepareHost(item, detail[index]));
    if (kind === "struct") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item, detail[key])]));
    if (kind === "tagged") {
      const fields = variants[value[detail]] ?? {};
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item, fields[key])]));
    }
  }
  if (Array.isArray(value)) return value.map((item: any) => prepareHost(item));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, prepareHost(item)]));
  }
  return value;
}

const bufferConstructors: Record<string, any> = {
  u8: Uint8Array, i8: Int8Array, u16: Uint16Array, i16: Int16Array,
  u32: Uint32Array, i32: Int32Array, u64: BigUint64Array, i64: BigInt64Array,
  f32: Float32Array, f64: Float64Array,
};

function restoreJson(value: any): any {
  if (typeof value === "bigint") {
    const number = Number(value);
    return Number.isSafeInteger(number) && BigInt(number) === value ? number : value;
  }
  if (Array.isArray(value)) return Object.freeze(value.map(restoreJson));
  if (value !== null && typeof value === "object") {
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreJson(item)])));
  }
  return value;
}

export function restoreHost(value: any, spec: any): any {
  if (value == null || spec == null) return value;
  const [kind, detail, variants] = spec;
  if (kind === "bytes") return value instanceof Uint8Array ? value : new Uint8Array(value);
  if (kind === "buffer") {
    const BufferType = bufferConstructors[detail];
    return value instanceof BufferType ? value : new BufferType(value);
  }
  if (kind === "json") return restoreJson(value);
  if (kind === "list") return Object.freeze(Array.from(value, (item: any) => restoreHost(item, detail)));
  if (kind === "map") return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail)])));
  if (kind === "tuple") return Object.freeze(value.map((item: any, index: number) => restoreHost(item, detail[index])));
  if (kind === "named") return restoreHost(value, nativeSchemas[detail]);
  if (kind === "alias") return restoreHost(value, detail);
  if (kind === "struct") return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail[key])])));
  if (kind === "tagged") {
    const fields = variants[value[detail]] ?? {};
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, fields[key])])));
  }
  return value;
}

export function nativeError(error: any, ErrorType: new (code: string, message: string) => Error): any {
  const text = String(error);
  const line = text.indexOf("\n");
  return line < 0 ? error : new ErrorType(text.slice(0, line), text.slice(line + 1));
}
"#;

// Callable and resource rendering -----------------------------------------

/// Render one free function or resource method wrapper.
fn emit_typescript_function(
    source: &mut String,
    function: &FunctionDef,
    context: &TypeScriptContext<'_>,
    receiver: Option<&str>,
) -> Result<()> {
    let params = typescript_params(&function.params, context)?;
    let calls = function
        .params
        .iter()
        .map(typescript_host_argument)
        .collect::<Result<Vec<_>>>()?
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
    let native_name = &function.native_name;
    let (indent, signature, call) = if receiver.is_some() {
        (
            "  ",
            format!(
                "  {}({params}): {} {{",
                function.host_name,
                return_type_ref(&function.returns, context)?
            ),
            format!("this.nativeResource.{}({calls})", function.host_name),
        )
    } else {
        (
            "",
            format!(
                "export function {}({params}): {} {{",
                function.host_name,
                return_type_ref(&function.returns, context)?
            ),
            format!("native[{quoted}]({calls})", quoted = ts_string(native_name)),
        )
    };
    source.push('\n');
    emit_ts_callable_doc(
        source,
        &CallableDocumentation {
            docs: function.docs.as_deref(),
            fallback_summary: format!("Calls the Rust implementation of `{}`.", function.host_name),
            params: &function.params,
            returns: CallableReturn::Contract(&function.returns),
            error: function.error.as_ref(),
        },
        context,
        indent,
    )?;
    writeln!(source, "{signature}")?;
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

/// Render one resource class, its factories, methods, and disposal hooks.
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
    let params = typescript_params(&constructor.params, context)?;
    let calls = constructor
        .params
        .iter()
        .map(typescript_host_argument)
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    source.push('\n');
    emit_ts_doc(
        source,
        Some(
            resource
                .docs
                .as_deref()
                .unwrap_or("A stateful resource implemented in Rust."),
        ),
        "",
    )?;
    writeln!(source, "export class {} {{", resource.name)?;
    source.push_str("  private nativeResource!: any;\n\n");
    emit_ts_callable_doc(
        source,
        &CallableDocumentation {
            docs: constructor.docs.as_deref(),
            fallback_summary: format!("Creates a `{}` instance.", resource.name),
            params: &constructor.params,
            returns: CallableReturn::Omitted,
            error: constructor.error.as_ref(),
        },
        context,
        "  ",
    )?;
    writeln!(source, "  constructor({params}) {{")?;
    let native_call = format!(
        "new native[{quoted}]({calls})",
        quoted = ts_string(&resource.native_name)
    );
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
        let params = typescript_params(&factory.params, context)?;
        let calls = factory
            .params
            .iter()
            .map(typescript_host_argument)
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        source.push('\n');
        emit_ts_callable_doc(
            source,
            &CallableDocumentation {
                docs: factory.docs.as_deref(),
                fallback_summary: format!("Creates a `{}` instance.", resource.name),
                params: &factory.params,
                returns: CallableReturn::Resource(&resource.name),
                error: factory.error.as_ref(),
            },
            context,
            "  ",
        )?;
        writeln!(
            source,
            "  static {}({params}): {} {{",
            factory.host_name, resource.name
        )?;
        writeln!(
            source,
            "    const value = Object.create({}.prototype);",
            resource.name
        )?;
        let native_call = format!(
            "native[{quoted}].{}({calls})",
            factory.host_name,
            quoted = ts_string(&resource.native_name)
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
            native_name: resource.native_name.clone(),
            docs: method.docs.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            error: method.error.clone(),
        };
        emit_typescript_function(source, &function, context, Some(&resource.name))?;
    }
    source.push('\n');
    emit_ts_callable_doc(
        source,
        &CallableDocumentation {
            docs: None,
            fallback_summary: "Releases the native resources owned by this instance.".to_owned(),
            params: &[],
            returns: CallableReturn::Omitted,
            error: None,
        },
        context,
        "  ",
    )?;
    source.push_str("  close(): void {\n    this.nativeResource.close();\n  }\n}\n");
    Ok(())
}

/// Render one Wasm-call argument, bypassing adapters for direct buffers.
fn typescript_host_argument(param: &ParamDef) -> Result<String> {
    Ok(format!(
        "prepareHost({}, {})",
        param.host_name,
        typescript_spec(&param.ty)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource_manifest() -> Manifest {
        let owner = rspyts::ir::CargoPackageId::new("test");
        let identity = rspyts::ir::DefinitionId::new("test", "test::Recorder");
        let constructor = |rust_name: &str, host_name: &str| FunctionDef {
            owner: owner.clone(),
            rust_module: "test".to_owned(),
            rust_name: rust_name.to_owned(),
            host_name: host_name.to_owned(),
            native_name: "native_recorder".to_owned(),
            docs: Some(format!("Create a recorder with `{rust_name}`.")),
            params: Vec::new(),
            returns: TypeRef::Named {
                identity: identity.clone(),
            },
            error: None,
        };
        Manifest {
            package_name: "test".to_owned(),
            package_version: "0.0.0".to_owned(),
            module_name: "native".to_owned(),
            types: Vec::new(),
            errors: Vec::new(),
            functions: Vec::new(),
            resources: vec![ResourceDef {
                owner: owner.clone(),
                rust_module: "test".to_owned(),
                name: "Recorder".to_owned(),
                native_name: "native_recorder".to_owned(),
                docs: Some("A stateful recorder.".to_owned()),
                constructors: vec![
                    constructor("new", "new"),
                    constructor("from_default", "fromDefault"),
                ],
                methods: Vec::new(),
            }],
            constants: Vec::new(),
        }
    }

    #[test]
    fn resource_factories_document_the_resource_without_model_resolution() {
        let manifest = resource_manifest();
        let namespace = Namespace::root();
        let context = TypeScriptContext {
            manifest: &manifest,
            namespace: &namespace,
        };
        let items = NamespaceItems {
            resources: vec![&manifest.resources[0]],
            ..NamespaceItems::default()
        };

        let source = typescript_api(&items, &context).expect("render resource factory");

        assert!(source.contains("static fromDefault(): Recorder"));
        assert!(source.contains("@returns A value typed as `Recorder`."));
    }
}
