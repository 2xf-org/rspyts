use super::*;

pub(super) fn typescript_api(
    items: &NamespaceItems<'_>,
    context: &TypeScriptContext<'_>,
) -> Result<String> {
    let runtime_imports = typescript_runtime_imports(items);
    let mut source = String::new();
    if items
        .types
        .iter()
        .any(|definition| matches!(&definition.shape, TypeShape::StringEnum { .. }))
    {
        writeln!(
            source,
            "import {{ freeze as $rspytsFreeze }} from {};",
            ts_string(&typescript_values_path(context.namespace))
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
            namespace_alias(&import),
            ts_string(&typescript_import(context.package, &import))
        )?;
    }
    for (index, definition) in items.types.iter().enumerate() {
        if let TypeShape::StringEnum { variants } = &definition.shape {
            let binding = format!("$rspytsStringEnum{index}");
            writeln!(source, "\nconst {binding} = $rspytsFreeze({{")?;
            for variant in variants {
                writeln!(
                    source,
                    "  [{}]: {},",
                    ts_string(&variant.rust_name),
                    ts_string(&variant.wire_name),
                )?;
            }
            writeln!(
                source,
                "}});\nexport {{ {binding} as {} }};",
                definition.name
            )?;
        }
    }
    for error in &items.errors {
        writeln!(
            source,
            "\nexport class {} extends globalThis.Error {{\n  constructor(code, message) {{\n    super(message);\n    this.name = {};\n    this.code = code;\n  }}\n}}",
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

pub(super) fn typescript_runtime(manifest: &Manifest) -> Result<String> {
    let mut source = String::from(
        "import initializeNative, * as native from \"./native.js\";\n\nconst wasmUrl = new URL(\"./native_bg.wasm\", import.meta.url);\nlet wasmInput = wasmUrl;\nif (globalThis.process?.versions?.node) {\n  const nodeModule = \"node:fs/promises\";\n  const { readFile } = await import(/* @vite-ignore */ nodeModule);\n  if (wasmUrl.protocol === \"file:\") {\n    wasmInput = await readFile(wasmUrl);\n  } else if (wasmUrl.pathname.startsWith(\"/@fs/\")) {\n    wasmInput = await readFile(decodeURIComponent(wasmUrl.pathname.slice(4)));\n  }\n}\nawait initializeNative({ module_or_path: wasmInput });\n\nexport { native };\n",
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

pub(super) const TYPESCRIPT_VALUES: &str = "export const freeze = Object.freeze;\n";

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

function restoreJson(value) {
  if (typeof value === "bigint") {
    const number = Number(value);
    if (!Number.isSafeInteger(number) || BigInt(number) !== value) {
      throw new RangeError("JSON integer exceeds JavaScript's safe integer range");
    }
    return number;
  }
  if (Array.isArray(value)) return Object.freeze(value.map(restoreJson));
  if (value !== null && typeof value === "object") {
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreJson(item)])));
  }
  return value;
}

export function restoreHost(value, spec) {
  if (value == null || spec == null) return value;
  const [kind, detail, variants] = spec;
  if (kind === "bytes") return new Uint8Array(value);
  if (kind === "buffer") return new bufferConstructors[detail](value);
  if (kind === "json") return restoreJson(value);
  if (kind === "list") return Object.freeze(Array.from(value, item => restoreHost(item, detail)));
  if (kind === "map") return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail)])));
  if (kind === "tuple") return Object.freeze(value.map((item, index) => restoreHost(item, detail[index])));
  if (kind === "named") return restoreHost(value, nativeSchemas[detail]);
  if (kind === "alias") return restoreHost(value, detail);
  if (kind === "struct") return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, detail[key])])));
  if (kind === "tagged") {
    const fields = variants[value[detail]] ?? {};
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, item]) => [key, restoreHost(item, fields[key])])));
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
    let native_name = &function.native_name;
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
            format!("native[{quoted}]({calls})", quoted = ts_string(native_name)),
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
    source.push_str("\n  close() {\n    this.nativeResource.close();\n  }\n}\n");
    Ok(())
}
