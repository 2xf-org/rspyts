use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};
use rspyts::ir::{
    BufferElement, DefinitionId, FieldDef, ScalarValue, Target, TypeDef, TypeRef, TypeShape,
};
use serde_json::Value;

use crate::config::{TypeScriptConfig, TypeScriptMode};
use crate::resolve::ResolvedContract;

use super::util::{
    TypeNames, error_definition, ordered_types, ts_doc, ts_property, type_allows_null,
    type_definition, type_names,
};
use super::{write, write_json};

pub fn emit(
    root: &Path,
    config: &TypeScriptConfig,
    contract: &ResolvedContract,
    fingerprint: &str,
) -> Result<()> {
    let manifest = &contract.manifest;
    let output = root.join("typescript");
    let names = type_names(contract);
    if config.mode == TypeScriptMode::Static {
        validate_static_contract(contract)?;
    }
    let main_declarations = declarations(contract, fingerprint, &names, config.mode);
    match config.mode {
        TypeScriptMode::Wasm => {
            let wire_constants = wire_constant_indices(contract);
            let wire_types = wire_type_ids(contract, &wire_constants);
            write(&output.join("index.d.ts"), &main_declarations)?;
            write(
                &output.join("index.js"),
                &wasm_runtime(contract, fingerprint),
            )?;
            write(
                &output.join("wire.d.ts"),
                &declarations_with_types(
                    contract,
                    fingerprint,
                    &names,
                    TypeScriptMode::Static,
                    Some(&wire_types),
                    Some(&wire_constants),
                ),
            )?;
            write(
                &output.join("wire.js"),
                &static_runtime_with_types(
                    contract,
                    fingerprint,
                    Some(&wire_types),
                    Some(&wire_constants),
                ),
            )?;
        }
        TypeScriptMode::Static => {
            write(&output.join("index.d.ts"), &main_declarations)?;
            write(
                &output.join("index.js"),
                &static_runtime(contract, fingerprint),
            )?;
        }
    }
    let peer_dependencies = typescript_peer_dependencies(contract, config.mode);
    let mut exports = serde_json::json!({
        ".": {
            "types": "./index.d.ts",
            "import": "./index.js",
            "default": "./index.js"
        },
        "./contract.json": "./contract.json"
    });
    let mut files = vec!["index.js", "index.d.ts", "contract.json"];
    if config.mode == TypeScriptMode::Wasm {
        exports
            .as_object_mut()
            .expect("exports is an object")
            .insert(
                "./wire".into(),
                serde_json::json!({
                    "types": "./wire.d.ts",
                    "import": "./wire.js",
                    "default": "./wire.js"
                }),
            );
        files.extend(["native.js", "native_bg.wasm", "wire.js", "wire.d.ts"]);
    }
    write_json(
        &output.join("package.json"),
        &serde_json::json!({
            "name": config.package,
            "version": manifest.crate_version,
            "type": "module",
            "main": "./index.js",
            "types": "./index.d.ts",
            "exports": exports,
            "files": files
            ,"peerDependencies": peer_dependencies
        }),
    )?;
    write_json(
        &output.join("contract.json"),
        &serde_json::json!({
            "schemaVersion": crate::LOCK_VERSION,
            "fingerprint": fingerprint,
            "hosts": contract.hosts,
            "dependencies": contract.dependencies,
            "manifest": manifest,
        }),
    )
}

const JAVASCRIPT_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn typescript_peer_dependencies(
    contract: &ResolvedContract,
    mode: TypeScriptMode,
) -> std::collections::BTreeMap<String, String> {
    let emitted_types = (mode == TypeScriptMode::Static).then(|| static_surface_type_ids(contract));
    contract
        .dependencies
        .values()
        .filter(|dependency| dependency_is_emitted(&dependency.owner, emitted_types.as_ref()))
        .filter_map(|dependency| {
            dependency.typescript.as_ref().and_then(|host| {
                typescript_dependency_specifier(host, mode)
                    .map(|_| (host.package.clone(), dependency.crate_version.clone()))
            })
        })
        .collect()
}

pub(super) fn validate_static_contract(contract: &ResolvedContract) -> Result<()> {
    if let Some(function) = contract
        .manifest
        .functions
        .iter()
        .find(|function| matches!(function.target, Target::Both | Target::Typescript))
    {
        bail!(
            "static TypeScript output cannot emit executable function `{}`; use `mode = \"wasm\"` or remove the TypeScript target",
            function.host_name
        );
    }
    if let Some(resource) = contract
        .manifest
        .resources
        .iter()
        .find(|resource| matches!(resource.target, Target::Both | Target::Typescript))
    {
        bail!(
            "static TypeScript output cannot emit executable resource `{}`; use `mode = \"wasm\"` or remove the TypeScript target",
            resource.name
        );
    }
    validate_static_types(contract)?;

    let emitted_types = static_surface_type_ids(contract);
    for definition in contract.manifest.types.iter().chain(
        contract
            .foreign_types
            .iter()
            .filter(|(identity, _)| emitted_types.contains(*identity))
            .map(|(_, definition)| definition),
    ) {
        let fields = match &definition.shape {
            TypeShape::Struct { fields } => fields.iter().collect::<Vec<_>>(),
            TypeShape::TaggedEnum { variants, .. } => variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        for field in fields {
            for (kind, value) in [
                ("literal", field.constraints.literal.as_ref()),
                ("default", field.default.as_ref()),
            ] {
                if let Some(value) = value {
                    let value = scalar_json(value);
                    let path = format!(
                        "static field `{}.{}` {kind}",
                        definition.name, field.wire_name
                    );
                    validate_static_value(&value, &field.ty, contract, &path)?;
                    validate_field_constraints(
                        &value,
                        &field.ty,
                        &field.constraints,
                        contract,
                        &path,
                    )?;
                }
            }
        }
    }
    validate_static_constants(contract)
}

fn validate_static_constants(contract: &ResolvedContract) -> Result<()> {
    for constant in contract
        .manifest
        .constants
        .iter()
        .filter(|constant| constant_in_typescript(constant.target, TypeScriptMode::Static))
    {
        validate_static_value(
            &constant.value,
            &constant.ty,
            contract,
            &format!("static constant `{}`", constant.host_name),
        )?;
    }
    Ok(())
}

pub(super) fn wire_constant_indices(contract: &ResolvedContract) -> BTreeSet<usize> {
    contract
        .manifest
        .constants
        .iter()
        .enumerate()
        .filter(|(_, constant)| {
            constant_in_typescript(constant.target, TypeScriptMode::Static)
                && validate_static_value(
                    &constant.value,
                    &constant.ty,
                    contract,
                    &format!("static constant `{}`", constant.host_name),
                )
                .is_ok()
        })
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn wire_type_ids(
    contract: &ResolvedContract,
    wire_constants: &BTreeSet<usize>,
) -> BTreeSet<DefinitionId> {
    let mut types = BTreeSet::new();
    for definition in &contract.manifest.types {
        if validate_static_type_definition(definition, contract).is_ok() {
            collect_type_definition_ids(definition, contract, &mut types);
        }
    }
    for constant in contract
        .manifest
        .constants
        .iter()
        .enumerate()
        .filter(|(index, _)| wire_constants.contains(index))
        .map(|(_, constant)| constant)
    {
        if validate_static_type_ref(
            &constant.ty,
            None,
            contract,
            &format!("static constant `{}`", constant.host_name),
            &mut BTreeSet::new(),
        )
        .is_ok()
        {
            collect_type_ref_ids(&constant.ty, contract, &mut types);
        }
    }
    types
}

pub(super) fn static_surface_type_ids(contract: &ResolvedContract) -> BTreeSet<DefinitionId> {
    let mut types = BTreeSet::new();
    for definition in &contract.manifest.types {
        collect_type_definition_ids(definition, contract, &mut types);
    }
    for constant in contract
        .manifest
        .constants
        .iter()
        .filter(|constant| constant_in_typescript(constant.target, TypeScriptMode::Static))
    {
        collect_type_ref_ids(&constant.ty, contract, &mut types);
    }
    types
}

fn collect_type_definition_ids(
    definition: &TypeDef,
    contract: &ResolvedContract,
    types: &mut BTreeSet<DefinitionId>,
) {
    let identity = DefinitionId {
        owner: definition.owner.clone(),
        id: definition.id.clone(),
    };
    if !types.insert(identity) {
        return;
    }
    match &definition.shape {
        TypeShape::Alias { target } => collect_type_ref_ids(target, contract, types),
        TypeShape::Struct { fields } => {
            for field in fields {
                collect_type_ref_ids(&field.ty, contract, types);
            }
        }
        TypeShape::TaggedEnum { variants, .. } => {
            for variant in variants {
                for field in &variant.fields {
                    collect_type_ref_ids(&field.ty, contract, types);
                }
            }
        }
        TypeShape::StringEnum { .. } => {}
    }
}

fn collect_type_ref_ids(
    reference: &TypeRef,
    contract: &ResolvedContract,
    types: &mut BTreeSet<DefinitionId>,
) {
    match reference {
        TypeRef::Option { item } | TypeRef::List { item } => {
            collect_type_ref_ids(item, contract, types);
        }
        TypeRef::Map { value } => collect_type_ref_ids(value, contract, types),
        TypeRef::Tuple { items } => {
            for item in items {
                collect_type_ref_ids(item, contract, types);
            }
        }
        TypeRef::Named { identity } => {
            if let Some(definition) = type_definition(contract, identity) {
                collect_type_definition_ids(definition, contract, types);
            }
        }
        _ => {}
    }
}

fn validate_static_types(contract: &ResolvedContract) -> Result<()> {
    let emitted_types = static_surface_type_ids(contract);
    for definition in contract.manifest.types.iter().chain(
        contract
            .foreign_types
            .iter()
            .filter(|(identity, _)| emitted_types.contains(*identity))
            .map(|(_, definition)| definition),
    ) {
        validate_static_type_definition(definition, contract)?;
    }
    Ok(())
}

fn validate_static_type_definition(
    definition: &TypeDef,
    contract: &ResolvedContract,
) -> Result<()> {
    match &definition.shape {
        TypeShape::Alias { target } => validate_static_type_ref(
            target,
            None,
            contract,
            &format!("static type `{}`", definition.name),
            &mut BTreeSet::new(),
        ),
        TypeShape::Struct { fields } => {
            for field in fields {
                validate_static_type_ref(
                    &field.ty,
                    Some(&field.constraints),
                    contract,
                    &format!("static field `{}.{}`", definition.name, field.wire_name),
                    &mut BTreeSet::new(),
                )?;
            }
            Ok(())
        }
        TypeShape::TaggedEnum { variants, .. } => {
            for variant in variants {
                for field in &variant.fields {
                    validate_static_type_ref(
                        &field.ty,
                        Some(&field.constraints),
                        contract,
                        &format!(
                            "static field `{}::{}.{}`",
                            definition.name, variant.wire_name, field.wire_name
                        ),
                        &mut BTreeSet::new(),
                    )?;
                }
            }
            Ok(())
        }
        TypeShape::StringEnum { .. } => Ok(()),
    }
}

fn validate_static_type_ref(
    reference: &TypeRef,
    constraints: Option<&rspyts::ir::FieldConstraints>,
    contract: &ResolvedContract,
    path: &str,
    visiting: &mut BTreeSet<DefinitionId>,
) -> Result<()> {
    match reference {
        TypeRef::Int { signed, bits: 64 } => {
            let safe_literal = constraints
                .and_then(|constraints| constraints.literal.as_ref())
                .and_then(|literal| match literal {
                    ScalarValue::I64(value) => Some(*value),
                    _ => None,
                })
                .is_some_and(|value| {
                    value
                        >= if *signed {
                            -(JAVASCRIPT_MAX_SAFE_INTEGER as i64)
                        } else {
                            0
                        }
                        && value <= JAVASCRIPT_MAX_SAFE_INTEGER as i64
                });
            if !safe_literal {
                let kind = if *signed { "i64" } else { "u64" };
                bail!(
                    "{path} exposes {kind} as a JavaScript number without a bounded safe literal; use a <=32-bit integer, add a safe `literal` constraint, or use the WASM bigint surface"
                );
            }
            Ok(())
        }
        TypeRef::Option { item } => {
            validate_static_type_ref(item, constraints, contract, path, visiting)
        }
        TypeRef::List { item } => validate_static_type_ref(item, None, contract, path, visiting),
        TypeRef::Map { value } => validate_static_type_ref(value, None, contract, path, visiting),
        TypeRef::Tuple { items } => {
            for item in items {
                validate_static_type_ref(item, None, contract, path, visiting)?;
            }
            Ok(())
        }
        TypeRef::Named { identity } => {
            if !visiting.insert(identity.clone()) {
                return Ok(());
            }
            let definition = type_definition(contract, identity)
                .ok_or_else(|| anyhow::anyhow!("{path} references missing type `{identity}`"))?;
            let result = match &definition.shape {
                TypeShape::Alias { target } => {
                    validate_static_type_ref(target, None, contract, path, visiting)
                }
                TypeShape::Struct { fields } => {
                    for field in fields {
                        validate_static_type_ref(
                            &field.ty,
                            Some(&field.constraints),
                            contract,
                            &format!("{path}.{}", field.wire_name),
                            visiting,
                        )?;
                    }
                    Ok(())
                }
                TypeShape::TaggedEnum { variants, .. } => {
                    for variant in variants {
                        for field in &variant.fields {
                            validate_static_type_ref(
                                &field.ty,
                                Some(&field.constraints),
                                contract,
                                &format!("{path}::{}.{}", variant.wire_name, field.wire_name),
                                visiting,
                            )?;
                        }
                    }
                    Ok(())
                }
                TypeShape::StringEnum { .. } => Ok(()),
            };
            visiting.remove(identity);
            result
        }
        TypeRef::Buffer {
            element: BufferElement::U64,
        } => bail!(
            "{path} exposes a u64 buffer as JavaScript numbers without per-item bounds; use a <=32-bit buffer element or the WASM bigint surface"
        ),
        TypeRef::Buffer {
            element: BufferElement::I64,
        } => bail!(
            "{path} exposes an i64 buffer as JavaScript numbers without per-item bounds; use a <=32-bit buffer element or the WASM bigint surface"
        ),
        TypeRef::Unit
        | TypeRef::Bool
        | TypeRef::Int { .. }
        | TypeRef::Float { .. }
        | TypeRef::String
        | TypeRef::DateTime
        | TypeRef::Json
        | TypeRef::Bytes
        | TypeRef::FixedBytes { .. }
        | TypeRef::Buffer { .. } => Ok(()),
    }
}

fn scalar_json(value: &ScalarValue) -> Value {
    match value {
        ScalarValue::Bool(value) => Value::Bool(*value),
        ScalarValue::I64(value) => Value::Number((*value).into()),
        ScalarValue::String(value) => Value::String(value.clone()),
    }
}

fn validate_static_value(
    value: &Value,
    ty: &TypeRef,
    contract: &ResolvedContract,
    path: &str,
) -> Result<()> {
    match ty {
        TypeRef::Unit => expect_kind(value, path, "null", Value::is_null),
        TypeRef::Bool => expect_kind(value, path, "boolean", Value::is_boolean),
        TypeRef::Int { signed, bits } => validate_integer(value, *signed, *bits, path),
        TypeRef::Float { bits } => validate_float(value, *bits, path),
        TypeRef::String | TypeRef::DateTime => expect_kind(value, path, "string", Value::is_string),
        TypeRef::Json => validate_json_value(value, path),
        TypeRef::Option { item } => {
            if value.is_null() {
                Ok(())
            } else {
                validate_static_value(value, item, contract, path)
            }
        }
        TypeRef::List { item } => {
            let values = expect_array(value, path)?;
            for (index, value) in values.iter().enumerate() {
                validate_static_value(value, item, contract, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        TypeRef::Map { value: item } => {
            let values = expect_object(value, path)?;
            for (key, value) in values {
                validate_static_value(
                    value,
                    item,
                    contract,
                    &format!("{path}[{}]", json_string(key)),
                )?;
            }
            Ok(())
        }
        TypeRef::Tuple { items } => {
            let values = expect_array(value, path)?;
            if values.len() != items.len() {
                bail!(
                    "{path} expected tuple with {} elements, found {}",
                    items.len(),
                    values.len()
                );
            }
            for (index, (value, item)) in values.iter().zip(items).enumerate() {
                validate_static_value(value, item, contract, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        TypeRef::Named { identity } => match type_definition(contract, identity) {
            Some(TypeDef {
                shape: TypeShape::Alias { target },
                ..
            }) => validate_static_value(value, target, contract, path),
            Some(TypeDef {
                shape: TypeShape::Struct { fields },
                ..
            }) => validate_struct(value, fields, None, contract, path),
            Some(TypeDef {
                shape: TypeShape::StringEnum { variants },
                ..
            }) => {
                let wire = value.as_str().ok_or_else(|| {
                    anyhow::anyhow!("{path} expected string enum, found {}", value_kind(value))
                })?;
                if variants.iter().any(|variant| variant.wire_name == wire) {
                    Ok(())
                } else {
                    bail!("{path} expected a declared string enum variant, found {wire:?}")
                }
            }
            Some(TypeDef {
                shape: TypeShape::TaggedEnum { tag, variants },
                ..
            }) => {
                let values = expect_object(value, path)?;
                let tag_path = format!("{path}[{}]", json_string(tag));
                let wire = values
                    .get(tag)
                    .ok_or_else(|| {
                        anyhow::anyhow!("{tag_path} expected required tag, found missing")
                    })?
                    .as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "{tag_path} expected variant string, found {}",
                            value_kind(&values[tag])
                        )
                    })?;
                let matching = variants
                    .iter()
                    .filter(|variant| variant.wire_name == wire)
                    .collect::<Vec<_>>();
                let [variant] = matching.as_slice() else {
                    if matching.is_empty() {
                        bail!("{tag_path} expected a declared tagged variant, found {wire:?}");
                    }
                    bail!(
                        "{tag_path} is ambiguous because variant {wire:?} is declared more than once"
                    );
                };
                if variant.fields.iter().any(|field| field.wire_name == *tag) {
                    bail!(
                        "{path} cannot validate tagged variant {wire:?}: field {tag:?} duplicates its tag"
                    );
                }
                validate_struct(value, &variant.fields, Some((tag, wire)), contract, path)
            }
            None => bail!("{path} references missing type `{identity}`"),
        },
        TypeRef::Bytes => {
            let values = expect_array(value, path)?;
            for (index, value) in values.iter().enumerate() {
                validate_integer(value, false, 8, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        TypeRef::FixedBytes { length } => {
            let values = expect_array(value, path)?;
            if values.len() as u64 != *length {
                bail!(
                    "{path} expected exactly {length} bytes, found {}",
                    values.len()
                );
            }
            for (index, value) in values.iter().enumerate() {
                validate_integer(value, false, 8, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        TypeRef::Buffer { element } => {
            let values = expect_array(value, path)?;
            for (index, value) in values.iter().enumerate() {
                let item_path = format!("{path}[{index}]");
                match element {
                    BufferElement::U8 => validate_integer(value, false, 8, &item_path)?,
                    BufferElement::I8 => validate_integer(value, true, 8, &item_path)?,
                    BufferElement::U16 => validate_integer(value, false, 16, &item_path)?,
                    BufferElement::I16 => validate_integer(value, true, 16, &item_path)?,
                    BufferElement::U32 => validate_integer(value, false, 32, &item_path)?,
                    BufferElement::I32 => validate_integer(value, true, 32, &item_path)?,
                    BufferElement::U64 => validate_integer(value, false, 64, &item_path)?,
                    BufferElement::I64 => validate_integer(value, true, 64, &item_path)?,
                    BufferElement::F32 => validate_float(value, 32, &item_path)?,
                    BufferElement::F64 => validate_float(value, 64, &item_path)?,
                }
            }
            Ok(())
        }
    }
}

fn validate_struct(
    value: &Value,
    fields: &[FieldDef],
    tag: Option<(&str, &str)>,
    contract: &ResolvedContract,
    path: &str,
) -> Result<()> {
    let values = expect_object(value, path)?;
    let allowed = fields
        .iter()
        .map(|field| field.wire_name.as_str())
        .chain(tag.map(|(tag, _)| tag))
        .collect::<BTreeSet<_>>();
    if let Some(key) = values.keys().find(|key| !allowed.contains(key.as_str())) {
        bail!(
            "{path}[{}] expected a declared field, found unknown field",
            json_string(key)
        );
    }
    for field in fields {
        let field_path = format!("{path}[{}]", json_string(&field.wire_name));
        match values.get(&field.wire_name) {
            Some(value) => {
                validate_static_value(value, &field.ty, contract, &field_path)?;
                validate_field_constraints(
                    value,
                    &field.ty,
                    &field.constraints,
                    contract,
                    &field_path,
                )?;
            }
            None if field.required => {
                bail!("{field_path} expected required field, found missing")
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_field_constraints(
    value: &Value,
    ty: &TypeRef,
    constraints: &rspyts::ir::FieldConstraints,
    contract: &ResolvedContract,
    path: &str,
) -> Result<()> {
    if value.is_null() && static_type_allows_null(ty, contract, &mut BTreeSet::new()) {
        return Ok(());
    }
    if let Some(literal) = &constraints.literal {
        let literal = scalar_json(literal);
        validate_static_value(&literal, ty, contract, path)?;
        if value != &literal {
            bail!("{path} expected declared literal {literal}, found {value}");
        }
    }
    if constraints.min_length.is_some() || constraints.max_length.is_some() {
        let length = match value {
            Value::String(value) => value.chars().count() as u64,
            Value::Array(value) => value.len() as u64,
            _ => bail!(
                "{path} expected string or array for length constraint, found {}",
                value_kind(value)
            ),
        };
        if let Some(minimum) = constraints.min_length
            && length < minimum
        {
            bail!("{path} expected length >= {minimum}, found length {length}");
        }
        if let Some(maximum) = constraints.max_length
            && length > maximum
        {
            bail!("{path} expected length <= {maximum}, found length {length}");
        }
    }
    if let Some(minimum) = constraints.ge {
        let passes = value
            .as_i64()
            .map(|value| value >= minimum)
            .or_else(|| {
                value
                    .as_u64()
                    .map(|value| minimum <= 0 || value >= minimum as u64)
            })
            .unwrap_or(false);
        if !passes {
            bail!("{path} expected integer >= {minimum}, found {value}");
        }
    }
    if let Some(maximum) = constraints.le {
        let passes = value
            .as_i64()
            .map(|value| value <= maximum)
            .or_else(|| {
                value
                    .as_u64()
                    .map(|value| maximum >= 0 && value <= maximum as u64)
            })
            .unwrap_or(false);
        if !passes {
            bail!("{path} expected integer <= {maximum}, found {value}");
        }
    }
    Ok(())
}

fn static_type_allows_null(
    reference: &TypeRef,
    contract: &ResolvedContract,
    visited: &mut BTreeSet<DefinitionId>,
) -> bool {
    match reference {
        TypeRef::Option { .. } => true,
        TypeRef::Named { identity } if visited.insert(identity.clone()) => {
            match type_definition(contract, identity).map(|definition| &definition.shape) {
                Some(TypeShape::Alias { target }) => {
                    static_type_allows_null(target, contract, visited)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn validate_integer(value: &Value, signed: bool, bits: u16, path: &str) -> Result<()> {
    let valid_bits = matches!(bits, 8 | 16 | 32 | 64);
    if !valid_bits {
        bail!("{path} uses unsupported {bits}-bit integer type");
    }
    if signed {
        let Some(value) = value.as_i64() else {
            bail!(
                "{path} expected signed {bits}-bit integer, found {}",
                value_kind(value)
            );
        };
        let (minimum, maximum) = match bits {
            8 => (i8::MIN as i64, i8::MAX as i64),
            16 => (i16::MIN as i64, i16::MAX as i64),
            32 => (i32::MIN as i64, i32::MAX as i64),
            64 => (i64::MIN, i64::MAX),
            _ => unreachable!(),
        };
        if value < minimum || value > maximum {
            bail!("{path} expected signed {bits}-bit integer, found {value}");
        }
        if bits == 64
            && (value < -(JAVASCRIPT_MAX_SAFE_INTEGER as i64)
                || value > JAVASCRIPT_MAX_SAFE_INTEGER as i64)
        {
            bail!("{path} contains i64 {value} outside JavaScript's safe integer range");
        }
    } else {
        let Some(value) = value.as_u64() else {
            bail!(
                "{path} expected unsigned {bits}-bit integer, found {}",
                value_kind(value)
            );
        };
        let maximum = match bits {
            8 => u8::MAX as u64,
            16 => u16::MAX as u64,
            32 => u32::MAX as u64,
            64 => u64::MAX,
            _ => unreachable!(),
        };
        if value > maximum {
            bail!("{path} expected unsigned {bits}-bit integer, found {value}");
        }
        if bits == 64 && value > JAVASCRIPT_MAX_SAFE_INTEGER {
            bail!("{path} contains u64 {value} outside JavaScript's safe integer range");
        }
    }
    Ok(())
}

fn validate_float(value: &Value, bits: u16, path: &str) -> Result<()> {
    if !matches!(bits, 32 | 64) {
        bail!("{path} uses unsupported {bits}-bit float type");
    }
    let Some(value) = value.as_f64() else {
        bail!(
            "{path} expected finite f{bits}, found {}",
            value_kind(value)
        );
    };
    if !value.is_finite() || bits == 32 && !(value as f32).is_finite() {
        bail!("{path} expected finite f{bits}, found {value}");
    }
    Ok(())
}

fn validate_json_value(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_json_value(value, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_json_value(value, &format!("{path}[{}]", json_string(key)))?;
            }
        }
        Value::Number(number) => {
            if let Some(value) = number.as_i64()
                && (value < -(JAVASCRIPT_MAX_SAFE_INTEGER as i64)
                    || value > JAVASCRIPT_MAX_SAFE_INTEGER as i64)
            {
                bail!(
                    "{path} contains JSON integer {value} outside JavaScript's safe integer range"
                );
            } else if let Some(value) = number.as_u64()
                && value > JAVASCRIPT_MAX_SAFE_INTEGER
            {
                bail!(
                    "{path} contains JSON integer {value} outside JavaScript's safe integer range"
                );
            } else {
                let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
                    bail!("{path} contains non-finite JSON number {number}");
                };
                if value.fract() == 0.0
                    && (value < -(JAVASCRIPT_MAX_SAFE_INTEGER as f64)
                        || value > JAVASCRIPT_MAX_SAFE_INTEGER as f64)
                {
                    bail!(
                        "{path} contains JSON integer {number} outside JavaScript's safe integer range"
                    );
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn expect_array<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{path} expected array, found {}", value_kind(value)))
}

fn expect_object<'a>(value: &'a Value, path: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{path} expected object, found {}", value_kind(value)))
}

fn expect_kind(
    value: &Value,
    path: &str,
    expected: &str,
    predicate: impl FnOnce(&Value) -> bool,
) -> Result<()> {
    if predicate(value) {
        Ok(())
    } else {
        bail!("{path} expected {expected}, found {}", value_kind(value))
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn declarations(
    contract: &ResolvedContract,
    fingerprint: &str,
    names: &TypeNames,
    mode: TypeScriptMode,
) -> String {
    if mode == TypeScriptMode::Static {
        let emitted_types = static_surface_type_ids(contract);
        declarations_with_types(
            contract,
            fingerprint,
            names,
            mode,
            Some(&emitted_types),
            None,
        )
    } else {
        declarations_with_types(contract, fingerprint, names, mode, None, None)
    }
}

fn declarations_with_types(
    contract: &ResolvedContract,
    fingerprint: &str,
    names: &TypeNames,
    mode: TypeScriptMode,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
    emitted_constants: Option<&BTreeSet<usize>>,
) -> String {
    let manifest = &contract.manifest;
    let mut output = String::from(
        "// Generated by rspyts 0.4. Do not edit.\n\
export type JsonValue = null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue };\n\n",
    );
    output.push_str(&typescript_declaration_imports(
        contract,
        mode,
        emitted_types,
    ));
    if mode == TypeScriptMode::Wasm {
        output.push_str(
            "export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;\n\
export interface InitOutput { readonly memory: WebAssembly.Memory; }\n\
export default function init(input?: InitInput | Promise<InitInput>): Promise<InitOutput>;\n\n",
        );
    }
    output.push_str(&format!(
        "export declare const CONTRACT_FINGERPRINT: {};\n\n",
        json_string(fingerprint)
    ));
    if mode == TypeScriptMode::Wasm {
        output.push_str(
            "export declare class ResourceClosedError extends globalThis.Error {\n  constructor(resource: string);\n}\n\n",
        );
    }

    for item in ordered_types(manifest).into_iter().filter(|definition| {
        emitted_types.is_none_or(|types| {
            types.contains(&DefinitionId {
                owner: definition.owner.clone(),
                id: definition.id.clone(),
            })
        })
    }) {
        output.push_str(&typescript_type(item, names, contract, mode));
        output.push('\n');
    }
    for error in manifest
        .errors
        .iter()
        .filter(|_| mode == TypeScriptMode::Wasm)
    {
        output.push_str(&ts_doc(error.docs.as_deref()));
        output.push_str(&format!(
            "export declare class {} extends globalThis.Error {{\n  readonly code: string;\n  constructor(message: string, code: string);\n}}\n\n",
            error.name
        ));
    }
    for function in manifest.functions.iter().filter(|function| {
        mode == TypeScriptMode::Wasm && matches!(function.target, Target::Both | Target::Typescript)
    }) {
        output.push_str(&ts_doc(function.docs.as_deref()));
        let declaration = if mode == TypeScriptMode::Wasm {
            "export function"
        } else {
            "export declare function"
        };
        output.push_str(&format!(
            "{declaration} {}({}): {};\n\n",
            function.host_name,
            function
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}: {}",
                        param.host_name,
                        typescript_ref(&param.ty, names, mode)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            typescript_ref(&function.returns, names, mode)
        ));
    }
    for resource in manifest.resources.iter().filter(|resource| {
        mode == TypeScriptMode::Wasm && matches!(resource.target, Target::Both | Target::Typescript)
    }) {
        output.push_str(&ts_doc(resource.docs.as_deref()));
        output.push_str(&format!("export declare class {} {{\n", resource.name));
        let constructors = resource
            .constructors
            .iter()
            .filter(|constructor| matches!(constructor.target, Target::Both | Target::Typescript))
            .collect::<Vec<_>>();
        let primary = constructors
            .iter()
            .copied()
            .find(|constructor| constructor.rust_name == "new")
            .or_else(|| constructors.first().copied());
        if let Some(constructor) = primary {
            output.push_str(&format!(
                "  constructor({});\n",
                constructor
                    .params
                    .iter()
                    .map(|param| format!(
                        "{}: {}",
                        param.host_name,
                        typescript_ref(&param.ty, names, mode)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for constructor in constructors
            .into_iter()
            .filter(|constructor| Some(*constructor) != primary)
        {
            output.push_str(&format!(
                "  static {}({}): {};\n",
                constructor.host_name,
                constructor
                    .params
                    .iter()
                    .map(|param| format!(
                        "{}: {}",
                        param.host_name,
                        typescript_ref(&param.ty, names, mode)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                resource.name,
            ));
        }
        for method in resource
            .methods
            .iter()
            .filter(|method| matches!(method.target, Target::Both | Target::Typescript))
        {
            output.push_str(&format!(
                "  {}({}): {};\n",
                method.host_name,
                method
                    .params
                    .iter()
                    .map(|param| format!(
                        "{}: {}",
                        param.host_name,
                        typescript_ref(&param.ty, names, mode)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                typescript_ref(&method.returns, names, mode)
            ));
        }
        output.push_str("  free(): void;\n}\n\n");
    }
    for constant in manifest
        .constants
        .iter()
        .enumerate()
        .filter(|(index, constant)| {
            constant_in_typescript(constant.target, mode)
                && emitted_constants.is_none_or(|indices| indices.contains(index))
        })
        .map(|(_, constant)| constant)
    {
        output.push_str(&ts_doc(constant.docs.as_deref()));
        output.push_str(&format!(
            "export const {}: {};\n",
            constant.host_name,
            typescript_literal_type(&constant.value, &constant.ty, contract, names, mode)
        ));
    }
    output
}

fn wasm_runtime(contract: &ResolvedContract, fingerprint: &str) -> String {
    let manifest = &contract.manifest;
    let mut output = format!(
        "// Generated by rspyts 0.4. Do not edit.\n{}\
import __rspyts_initialize_native, * as __rspyts_native from \"./native.js\";\n\n\
export const CONTRACT_FINGERPRINT = {};\n\n{}\
let __rspyts_initialization_promise;\n\n\
export default function init(__rspyts_input) {{\n\
  if (__rspyts_initialization_promise === void 0) {{\n\
    __rspyts_initialization_promise = globalThis.Promise.resolve()\n\
      .then(() => __rspyts_initialize_native({{ module_or_path: __rspyts_input }}))\n\
      .catch((__rspyts_error) => {{\n\
        __rspyts_initialization_promise = void 0;\n\
        throw __rspyts_error;\n\
      }});\n\
  }}\n\
  return __rspyts_initialization_promise;\n\
}}\n\n\
export class ResourceClosedError extends globalThis.Error {{\n\
  constructor(__rspyts_resource) {{\n\
    super(`${{__rspyts_resource}} is closed`);\n\
    this.name = \"ResourceClosedError\";\n\
  }}\n\
}}\n\n",
        typescript_runtime_imports(contract, TypeScriptMode::Wasm, None),
        json_string(fingerprint),
        dependency_fingerprint_assertions(contract, TypeScriptMode::Wasm, None)
    );
    output.push_str(deep_freeze_runtime());
    for item in &manifest.types {
        if let TypeShape::StringEnum { variants } = &item.shape {
            output.push_str(&format!(
                "export const {} = globalThis.Object.freeze({{\n",
                item.name
            ));
            for variant in variants {
                output.push_str(&format!(
                    "  {}: {},\n",
                    javascript_property(&variant.rust_name),
                    json_string(&variant.wire_name)
                ));
            }
            output.push_str("});\n\n");
        }
    }
    for error in &manifest.errors {
        output.push_str(&format!(
            "export class {} extends globalThis.Error {{\n  constructor(__rspyts_message, __rspyts_code) {{\n    super(__rspyts_message);\n    this.name = {};\n    this.code = __rspyts_code;\n  }}\n}}\n\n",
            error.name,
            json_string(&error.name)
        ));
    }
    for (index, error) in manifest
        .errors
        .iter()
        .chain(contract.foreign_errors.values())
        .enumerate()
    {
        output.push_str(&format!(
            "const __rspyts_error_class_{index} = {};\n",
            error.name
        ));
    }
    if !manifest.errors.is_empty() || !contract.foreign_errors.is_empty() {
        output.push('\n');
    }
    output.push_str(
        "function __rspyts_is_runtime_error(__rspyts_error) {\n\
  return __rspyts_error instanceof globalThis.WebAssembly.RuntimeError;\n\
}\n\n\
function __rspyts_create_resource(__rspyts_prototype) {\n\
  return globalThis.Object.create(__rspyts_prototype);\n\
}\n\n\
function __rspyts_translate_error(__rspyts_error, __rspyts_expected_type, __rspyts_error_class, __rspyts_codes = []) {\n\
  if (__rspyts_is_runtime_error(__rspyts_error) || __rspyts_error instanceof globalThis.Error && !__rspyts_error_class) {\n\
    return __rspyts_error;\n\
  }\n\
  const __rspyts_type_id = __rspyts_error?.typeId ?? __rspyts_error?.type_id;\n\
  let __rspyts_code = __rspyts_error?.code;\n\
  let __rspyts_message = __rspyts_error?.message;\n\
  if (typeof __rspyts_error === \"string\") {\n\
    const __rspyts_separator = __rspyts_error.indexOf(\"\\n\");\n\
    __rspyts_code = __rspyts_separator < 0 ? __rspyts_error : __rspyts_error.slice(0, __rspyts_separator);\n\
    __rspyts_message = __rspyts_separator < 0 ? __rspyts_error : __rspyts_error.slice(__rspyts_separator + 1);\n\
  }\n\
  if (__rspyts_error_class && (__rspyts_type_id === __rspyts_expected_type || __rspyts_codes.includes(__rspyts_code))) {\n\
    return new __rspyts_error_class(__rspyts_message ?? globalThis.String(__rspyts_error), __rspyts_code ?? \"unknown\");\n\
  }\n\
  return __rspyts_error instanceof globalThis.Error ? __rspyts_error : new globalThis.Error(__rspyts_error?.message ?? globalThis.String(__rspyts_error));\n\
}\n\n",
    );
    for function in manifest
        .functions
        .iter()
        .filter(|function| matches!(function.target, Target::Both | Target::Typescript))
    {
        let params = function
            .params
            .iter()
            .map(|param| param.host_name.as_str())
            .collect::<Vec<_>>();
        output.push_str(&format!(
            "export function {}({}) {{\n  try {{\n    return __rspyts_deep_freeze(__rspyts_native.{}({}));\n  }} catch (__rspyts_error) {{\n    throw {};\n  }}\n}}\n\n",
            function.host_name,
            params.join(", "),
            native_export_name(&function.host_name),
            params.join(", "),
            translate_error(&function.error, contract)
        ));
    }
    for resource in manifest
        .resources
        .iter()
        .filter(|resource| matches!(resource.target, Target::Both | Target::Typescript))
    {
        let constructors = resource
            .constructors
            .iter()
            .filter(|constructor| matches!(constructor.target, Target::Both | Target::Typescript))
            .collect::<Vec<_>>();
        let constructor = constructors
            .iter()
            .copied()
            .find(|constructor| constructor.rust_name == "new")
            .or_else(|| constructors.first().copied());
        let params = constructor
            .map(|constructor| {
                constructor
                    .params
                    .iter()
                    .map(|param| param.host_name.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        output.push_str(&format!(
            "export class {} {{\n  constructor({}) {{\n    try {{\n      this.__rspyts_handle = new __rspyts_native.__RspytsWasm{}({});\n    }} catch (__rspyts_error) {{\n      throw {};\n    }}\n  }}\n\n  __rspyts_require_handle() {{\n    if (this.__rspyts_handle === null) throw new ResourceClosedError({});\n    return this.__rspyts_handle;\n  }}\n",
            resource.name,
            params.join(", "),
            resource.name,
            params.join(", "),
            constructor
                .map(|constructor| translate_error(&constructor.error, contract))
                .unwrap_or_else(|| "__rspyts_translate_error(__rspyts_error)".into()),
            json_string(&resource.name)
        ));
        for factory in constructors
            .into_iter()
            .filter(|factory| Some(*factory) != constructor)
        {
            let params = factory
                .params
                .iter()
                .map(|param| param.host_name.as_str())
                .collect::<Vec<_>>();
            output.push_str(&format!(
                "\n  static {}({}) {{\n    try {{\n      const __rspyts_resource = __rspyts_create_resource(this.prototype);\n      __rspyts_resource.__rspyts_handle = __rspyts_native.__RspytsWasm{}.{}({});\n      return __rspyts_resource;\n    }} catch (__rspyts_error) {{\n      throw {};\n    }}\n  }}\n",
                factory.host_name,
                params.join(", "),
                resource.name,
                native_export_name(&factory.host_name),
                params.join(", "),
                translate_error(&factory.error, contract),
            ));
        }
        for method in resource
            .methods
            .iter()
            .filter(|method| matches!(method.target, Target::Both | Target::Typescript))
        {
            let params = method
                .params
                .iter()
                .map(|param| param.host_name.as_str())
                .collect::<Vec<_>>();
            output.push_str(&format!(
                "\n  {}({}) {{\n    try {{\n      return __rspyts_deep_freeze(this.__rspyts_require_handle().{}({}));\n    }} catch (__rspyts_error) {{\n      if (__rspyts_is_runtime_error(__rspyts_error)) this.free();\n      throw {};\n    }}\n  }}\n",
                method.host_name,
                params.join(", "),
                native_export_name(&method.host_name),
                params.join(", "),
                translate_error(&method.error, contract)
            ));
        }
        output.push_str(
            "\n  free() {\n    if (this.__rspyts_handle !== null) {\n      this.__rspyts_handle.free();\n      this.__rspyts_handle = null;\n    }\n  }\n}\n\n",
        );
    }
    for constant in manifest
        .constants
        .iter()
        .filter(|constant| constant_in_typescript(constant.target, TypeScriptMode::Wasm))
    {
        output.push_str(&format!(
            "export const {} = __rspyts_deep_freeze({});\n",
            constant.host_name,
            typescript_value(
                &constant.value,
                &constant.ty,
                contract,
                TypeScriptMode::Wasm,
            )
        ));
    }
    output
}

fn translate_error(error_id: &Option<DefinitionId>, contract: &ResolvedContract) -> String {
    match error_id
        .as_ref()
        .and_then(|identity| error_definition(contract, identity))
    {
        Some(error) => format!(
            "__rspyts_translate_error(__rspyts_error, {}, {}, [{}])",
            json_string(
                &DefinitionId {
                    owner: error.owner.clone(),
                    id: error.id.clone(),
                }
                .to_string()
            ),
            typescript_error_runtime_alias(contract, error_id.as_ref().expect("matched error")),
            error
                .variants
                .iter()
                .map(|variant| json_string(&variant.code))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => "__rspyts_translate_error(__rspyts_error)".into(),
    }
}

fn typescript_error_runtime_alias(contract: &ResolvedContract, identity: &DefinitionId) -> String {
    let index = contract
        .manifest
        .errors
        .iter()
        .position(|error| error.owner == identity.owner && error.id == identity.id)
        .or_else(|| {
            contract
                .foreign_errors
                .keys()
                .position(|candidate| candidate == identity)
                .map(|index| contract.manifest.errors.len() + index)
        })
        .expect("translated errors are resolved before TypeScript emission");
    format!("__rspyts_error_class_{index}")
}

fn native_export_name(host_name: &str) -> String {
    format!("__rspyts_export_{host_name}")
}

fn static_runtime(contract: &ResolvedContract, fingerprint: &str) -> String {
    let emitted_types = static_surface_type_ids(contract);
    static_runtime_with_types(contract, fingerprint, Some(&emitted_types), None)
}

fn static_runtime_with_types(
    contract: &ResolvedContract,
    fingerprint: &str,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
    emitted_constants: Option<&BTreeSet<usize>>,
) -> String {
    let manifest = &contract.manifest;
    let mut output = format!(
        "// Generated by rspyts 0.4. Do not edit.\n{}export const CONTRACT_FINGERPRINT = {};\n\n{}",
        typescript_runtime_imports(contract, TypeScriptMode::Static, emitted_types),
        json_string(fingerprint),
        dependency_fingerprint_assertions(contract, TypeScriptMode::Static, emitted_types)
    );
    output.push_str(deep_freeze_runtime());
    for item in manifest.types.iter().filter(|definition| {
        emitted_types.is_none_or(|types| {
            types.contains(&DefinitionId {
                owner: definition.owner.clone(),
                id: definition.id.clone(),
            })
        })
    }) {
        if let TypeShape::StringEnum { variants } = &item.shape {
            output.push_str(&format!(
                "export const {} = globalThis.Object.freeze({{\n",
                item.name
            ));
            for variant in variants {
                output.push_str(&format!(
                    "  {}: {},\n",
                    javascript_property(&variant.rust_name),
                    json_string(&variant.wire_name)
                ));
            }
            output.push_str("});\n\n");
        }
    }
    for constant in manifest
        .constants
        .iter()
        .enumerate()
        .filter(|(index, constant)| {
            constant_in_typescript(constant.target, TypeScriptMode::Static)
                && emitted_constants.is_none_or(|indices| indices.contains(index))
        })
        .map(|(_, constant)| constant)
    {
        output.push_str(&format!(
            "export const {} = __rspyts_deep_freeze({});\n",
            constant.host_name,
            typescript_value(
                &constant.value,
                &constant.ty,
                contract,
                TypeScriptMode::Static,
            )
        ));
    }
    output
}

fn deep_freeze_runtime() -> &'static str {
    "function __rspyts_deep_freeze(__rspyts_value, __rspyts_seen = new globalThis.WeakSet()) {\n\
  if (__rspyts_value === null || typeof __rspyts_value !== \"object\" || globalThis.ArrayBuffer.isView(__rspyts_value)) return __rspyts_value;\n\
  if (__rspyts_seen.has(__rspyts_value)) return __rspyts_value;\n\
  __rspyts_seen.add(__rspyts_value);\n\
  for (const __rspyts_nested of globalThis.Object.values(__rspyts_value)) __rspyts_deep_freeze(__rspyts_nested, __rspyts_seen);\n\
  return globalThis.Object.isFrozen(__rspyts_value) ? __rspyts_value : globalThis.Object.freeze(__rspyts_value);\n\
}\n\n"
}

fn constant_in_typescript(target: Target, mode: TypeScriptMode) -> bool {
    matches!(target, Target::Both | Target::Typescript)
        || target == Target::Static && mode == TypeScriptMode::Static
}

fn typescript_type(
    item: &TypeDef,
    names: &TypeNames,
    contract: &ResolvedContract,
    mode: TypeScriptMode,
) -> String {
    let mut output = ts_doc(item.docs.as_deref());
    match &item.shape {
        TypeShape::Struct { fields } => {
            output.push_str(&format!("export interface {} {{\n", item.name));
            for field in fields {
                if let Some(docs) = field.docs.as_deref() {
                    output.push_str("  ");
                    output.push_str(ts_doc(Some(docs)).replace('\n', "\n  ").trim_end());
                    output.push('\n');
                }
                output.push_str(&format!(
                    "  readonly {}{}: {};\n",
                    ts_property(&field.wire_name),
                    if field.required { "" } else { "?" },
                    typescript_field_ref(field, names, contract, mode)
                ));
            }
            output.push_str("}\n");
        }
        TypeShape::StringEnum { variants } => {
            output.push_str(&format!("export enum {} {{\n", item.name));
            for variant in variants {
                output.push_str(&format!(
                    "  {} = {},\n",
                    variant.rust_name,
                    json_string(&variant.wire_name)
                ));
            }
            output.push_str("}\n");
        }
        TypeShape::TaggedEnum { tag, variants } => {
            output.push_str(&format!("export type {} =\n", item.name));
            for (index, variant) in variants.iter().enumerate() {
                output.push_str(if index == 0 { "  " } else { "  | " });
                output.push_str(&format!(
                    "{{ readonly {}: {}",
                    ts_property(tag),
                    json_string(&variant.wire_name)
                ));
                for field in &variant.fields {
                    output.push_str(&format!(
                        "; readonly {}{}: {}",
                        ts_property(&field.wire_name),
                        if field.required { "" } else { "?" },
                        typescript_field_ref(field, names, contract, mode)
                    ));
                }
                output.push_str(" }\n");
            }
            output.push_str(";\n");
        }
        TypeShape::Alias { target } => output.push_str(&format!(
            "export type {} = {};\n",
            item.name,
            typescript_ref(target, names, mode)
        )),
    }
    output
}

fn typescript_field_ref(
    field: &FieldDef,
    names: &TypeNames,
    contract: &ResolvedContract,
    mode: TypeScriptMode,
) -> String {
    match &field.constraints.literal {
        Some(value) => {
            let literal = typescript_scalar(value, &field.ty, contract, mode);
            if type_allows_null(&field.ty, contract) {
                format!("{literal} | null")
            } else {
                literal
            }
        }
        None => typescript_ref(&field.ty, names, mode),
    }
}

fn typescript_ref(reference: &TypeRef, names: &TypeNames, mode: TypeScriptMode) -> String {
    match reference {
        TypeRef::Unit if mode == TypeScriptMode::Static => "null".into(),
        TypeRef::Unit => "void".into(),
        TypeRef::Bool => "boolean".into(),
        TypeRef::Int { bits: 64, .. } if mode == TypeScriptMode::Wasm => "bigint".into(),
        TypeRef::Int { .. } | TypeRef::Float { .. } => "number".into(),
        TypeRef::String => "string".into(),
        TypeRef::DateTime => "string".into(),
        TypeRef::Json => "JsonValue".into(),
        TypeRef::Option { item } => format!("{} | null", typescript_ref(item, names, mode)),
        TypeRef::List { item } => {
            format!("readonly ({})[]", typescript_ref(item, names, mode))
        }
        TypeRef::Map { value } => {
            format!(
                "{{ readonly [key: string]: {} }}",
                typescript_ref(value, names, mode)
            )
        }
        TypeRef::Tuple { items } => format!(
            "readonly [{}]",
            items
                .iter()
                .map(|item| typescript_ref(item, names, mode))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Named { identity } => names
            .get(identity)
            .cloned()
            .unwrap_or_else(|| identity.to_string()),
        TypeRef::Bytes if mode == TypeScriptMode::Static => "readonly number[]".into(),
        TypeRef::Bytes => "globalThis.Uint8Array".into(),
        TypeRef::FixedBytes { length } if mode == TypeScriptMode::Static => {
            format!("readonly number[] & {{ readonly length: {length} }}")
        }
        TypeRef::FixedBytes { .. } => "globalThis.Uint8Array".into(),
        TypeRef::Buffer { .. } if mode == TypeScriptMode::Static => "readonly number[]".into(),
        TypeRef::Buffer { element } => match element {
            BufferElement::U8 => "globalThis.Uint8Array",
            BufferElement::I8 => "globalThis.Int8Array",
            BufferElement::U16 => "globalThis.Uint16Array",
            BufferElement::I16 => "globalThis.Int16Array",
            BufferElement::U32 => "globalThis.Uint32Array",
            BufferElement::I32 => "globalThis.Int32Array",
            BufferElement::U64 => "globalThis.BigUint64Array",
            BufferElement::I64 => "globalThis.BigInt64Array",
            BufferElement::F32 => "globalThis.Float32Array",
            BufferElement::F64 => "globalThis.Float64Array",
        }
        .into(),
    }
}

fn typescript_literal_type(
    value: &Value,
    ty: &TypeRef,
    contract: &ResolvedContract,
    names: &TypeNames,
    mode: TypeScriptMode,
) -> String {
    match (value, ty) {
        (Value::Null, _) => "null".into(),
        (value, TypeRef::Option { item }) => {
            typescript_literal_type(value, item, contract, names, mode)
        }
        (value, TypeRef::Named { identity }) => match type_definition(contract, identity) {
            Some(TypeDef {
                name,
                shape: TypeShape::StringEnum { variants },
                ..
            }) => value
                .as_str()
                .and_then(|wire| {
                    variants
                        .iter()
                        .find(|variant| variant.wire_name == wire)
                        .map(|variant| format!("{name}.{}", variant.rust_name))
                })
                .unwrap_or_else(|| typescript_ref(ty, names, mode)),
            Some(TypeDef {
                shape: TypeShape::Alias { target },
                ..
            }) => typescript_literal_type(value, target, contract, names, mode),
            Some(TypeDef {
                shape: TypeShape::Struct { fields },
                ..
            }) => match value {
                Value::Object(values) => {
                    typescript_struct_literal_type(values, fields, contract, names, mode)
                }
                _ => typescript_ref(ty, names, mode),
            },
            Some(TypeDef {
                shape: TypeShape::TaggedEnum { tag, variants },
                ..
            }) => match value {
                Value::Object(values) => {
                    let variant = values
                        .get(tag)
                        .and_then(Value::as_str)
                        .and_then(|wire| variants.iter().find(|variant| variant.wire_name == wire));
                    variant
                        .map(|variant| {
                            let fields = std::iter::once(rspyts::ir::FieldDef {
                                rust_name: tag.clone(),
                                wire_name: tag.clone(),
                                docs: None,
                                ty: TypeRef::String,
                                required: true,
                                default: None,
                                constraints: rspyts::ir::FieldConstraints::default(),
                            })
                            .chain(variant.fields.iter().cloned())
                            .collect::<Vec<_>>();
                            typescript_struct_literal_type(values, &fields, contract, names, mode)
                        })
                        .unwrap_or_else(|| typescript_ref(ty, names, mode))
                }
                _ => typescript_ref(ty, names, mode),
            },
            None => typescript_ref(ty, names, mode),
        },
        (Value::Bool(value), _) => value.to_string(),
        (Value::Number(value), TypeRef::Int { bits: 64, .. }) if mode == TypeScriptMode::Wasm => {
            format!("{value}n")
        }
        (Value::Number(value), _) => value.to_string(),
        (Value::String(value), _) => json_string(value),
        (Value::Array(values), TypeRef::List { item }) => format!(
            "readonly [{}]",
            values
                .iter()
                .map(|value| typescript_literal_type(value, item, contract, names, mode))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Value::Array(values), TypeRef::Tuple { items }) => format!(
            "readonly [{}]",
            values
                .iter()
                .zip(items)
                .map(|(value, item)| {
                    typescript_literal_type(value, item, contract, names, mode)
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Value::Object(values), TypeRef::Map { value }) => format!(
            "{{ {} }}",
            values
                .iter()
                .map(|(key, item)| format!(
                    "readonly {}: {}",
                    json_string(key),
                    typescript_literal_type(item, value, contract, names, mode)
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        (
            Value::Array(values),
            TypeRef::Bytes | TypeRef::FixedBytes { .. } | TypeRef::Buffer { .. },
        ) if mode == TypeScriptMode::Static => {
            format!(
                "readonly [{}]",
                values
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        (Value::Array(_), TypeRef::Bytes | TypeRef::FixedBytes { .. } | TypeRef::Buffer { .. }) => {
            typescript_ref(ty, names, mode)
        }
        _ => typescript_json_literal_type(value, contract, names, mode),
    }
}

fn typescript_scalar(
    value: &ScalarValue,
    ty: &TypeRef,
    contract: &ResolvedContract,
    mode: TypeScriptMode,
) -> String {
    if let TypeRef::Option { item } = ty {
        return typescript_scalar(value, item, contract, mode);
    }
    if let TypeRef::Named { identity } = ty {
        return match type_definition(contract, identity) {
            Some(TypeDef {
                name,
                shape: TypeShape::StringEnum { variants },
                ..
            }) => match value {
                ScalarValue::String(value) => variants
                    .iter()
                    .find(|variant| variant.wire_name == *value)
                    .map(|variant| format!("{name}.{}", variant.rust_name))
                    .unwrap_or_else(|| json_string(value)),
                _ => typescript_scalar_value(value, ty, mode),
            },
            Some(TypeDef {
                shape: TypeShape::Alias { target },
                ..
            }) => typescript_scalar(value, target, contract, mode),
            _ => typescript_scalar_value(value, ty, mode),
        };
    }
    typescript_scalar_value(value, ty, mode)
}

fn typescript_scalar_value(value: &ScalarValue, ty: &TypeRef, mode: TypeScriptMode) -> String {
    match value {
        ScalarValue::Bool(value) => value.to_string(),
        ScalarValue::I64(value)
            if mode == TypeScriptMode::Wasm && matches!(ty, TypeRef::Int { bits: 64, .. }) =>
        {
            format!("{value}n")
        }
        ScalarValue::I64(value) => value.to_string(),
        ScalarValue::String(value) => json_string(value),
    }
}

fn typescript_struct_literal_type(
    values: &serde_json::Map<String, Value>,
    fields: &[rspyts::ir::FieldDef],
    contract: &ResolvedContract,
    names: &TypeNames,
    mode: TypeScriptMode,
) -> String {
    format!(
        "{{ {} }}",
        fields
            .iter()
            .filter_map(|field| {
                values.get(&field.wire_name).map(|value| {
                    format!(
                        "readonly {}: {}",
                        json_string(&field.wire_name),
                        typescript_literal_type(value, &field.ty, contract, names, mode)
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("; ")
    )
}

fn typescript_json_literal_type(
    value: &Value,
    contract: &ResolvedContract,
    names: &TypeNames,
    mode: TypeScriptMode,
) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => json_string(value),
        Value::Array(values) => format!(
            "readonly [{}]",
            values
                .iter()
                .map(|value| {
                    typescript_literal_type(value, &TypeRef::Json, contract, names, mode)
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{ {} }}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "readonly {}: {}",
                    json_string(key),
                    typescript_literal_type(value, &TypeRef::Json, contract, names, mode)
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

fn typescript_value(
    value: &Value,
    ty: &TypeRef,
    contract: &ResolvedContract,
    mode: TypeScriptMode,
) -> String {
    match (value, ty) {
        (Value::Number(number), TypeRef::Int { bits: 64, .. }) if mode == TypeScriptMode::Wasm => {
            format!("{number}n")
        }
        (Value::Array(values), TypeRef::List { item }) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| typescript_value(value, item, contract, mode))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Value::Array(values), TypeRef::Tuple { items }) => format!(
            "[{}]",
            values
                .iter()
                .zip(items)
                .map(|(value, item)| typescript_value(value, item, contract, mode))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Value::Object(values), TypeRef::Map { value }) => format!(
            "{{ {} }}",
            values
                .iter()
                .map(|(key, item)| format!(
                    "{}: {}",
                    javascript_property(key),
                    typescript_value(item, value, contract, mode)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (value, TypeRef::Option { item }) if !value.is_null() => {
            typescript_value(value, item, contract, mode)
        }
        (value, TypeRef::Named { identity }) => match type_definition(contract, identity) {
            Some(TypeDef {
                shape: TypeShape::Alias { target },
                ..
            }) => typescript_value(value, target, contract, mode),
            Some(TypeDef {
                shape: TypeShape::Struct { fields },
                ..
            }) => match value.as_object() {
                Some(values) => format!(
                    "{{ {} }}",
                    fields
                        .iter()
                        .filter_map(|field| {
                            values.get(&field.wire_name).map(|value| {
                                format!(
                                    "{}: {}",
                                    javascript_property(&field.wire_name),
                                    typescript_value(value, &field.ty, contract, mode)
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => serde_json::to_string(value).expect("JSON value is serializable"),
            },
            Some(TypeDef {
                shape: TypeShape::TaggedEnum { tag, variants },
                ..
            }) => match value.as_object() {
                Some(values) => {
                    let variant = values
                        .get(tag)
                        .and_then(Value::as_str)
                        .and_then(|tag| variants.iter().find(|variant| variant.wire_name == tag));
                    match variant {
                        Some(variant) => format!(
                            "{{ {} }}",
                            std::iter::once(format!(
                                "{}: {}",
                                javascript_property(tag),
                                json_string(&variant.wire_name)
                            ))
                            .chain(variant.fields.iter().filter_map(|field| {
                                values.get(&field.wire_name).map(|value| {
                                    format!(
                                        "{}: {}",
                                        javascript_property(&field.wire_name),
                                        typescript_value(value, &field.ty, contract, mode)
                                    )
                                })
                            }))
                            .collect::<Vec<_>>()
                            .join(", ")
                        ),
                        None => serde_json::to_string(value).expect("JSON value is serializable"),
                    }
                }
                None => serde_json::to_string(value).expect("JSON value is serializable"),
            },
            _ => serde_json::to_string(value).expect("JSON value is serializable"),
        },
        (Value::Array(values), TypeRef::Bytes | TypeRef::FixedBytes { .. })
            if mode == TypeScriptMode::Static =>
        {
            format!(
                "[{}]",
                values
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        (Value::Array(values), TypeRef::Bytes | TypeRef::FixedBytes { .. }) => format!(
            "new globalThis.Uint8Array([{}])",
            values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Value::Array(values), TypeRef::Buffer { .. }) if mode == TypeScriptMode::Static => {
            format!(
                "[{}]",
                values
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        (Value::Array(values), TypeRef::Buffer { element }) => format!(
            "new {}([{}])",
            match element {
                BufferElement::U8 => "globalThis.Uint8Array",
                BufferElement::I8 => "globalThis.Int8Array",
                BufferElement::U16 => "globalThis.Uint16Array",
                BufferElement::I16 => "globalThis.Int16Array",
                BufferElement::U32 => "globalThis.Uint32Array",
                BufferElement::I32 => "globalThis.Int32Array",
                BufferElement::U64 => "globalThis.BigUint64Array",
                BufferElement::I64 => "globalThis.BigInt64Array",
                BufferElement::F32 => "globalThis.Float32Array",
                BufferElement::F64 => "globalThis.Float64Array",
            },
            values
                .iter()
                .map(|value| {
                    if matches!(element, BufferElement::U64 | BufferElement::I64) {
                        format!("{value}n")
                    } else {
                        value.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (_, TypeRef::Json) => javascript_json_value(value),
        _ => serde_json::to_string(value).expect("JSON value is serializable"),
    }
}

fn javascript_json_value(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(javascript_json_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{ {} }}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    javascript_property(key),
                    javascript_json_value(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => serde_json::to_string(value).expect("JSON value is serializable"),
    }
}

fn javascript_property(value: &str) -> String {
    format!("[{}]", json_string(value))
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is serializable")
}

fn typescript_dependency_specifier(
    host: &crate::LockedTypeScriptHost,
    root_mode: TypeScriptMode,
) -> Option<String> {
    match (root_mode, host.mode) {
        (TypeScriptMode::Static, TypeScriptMode::Wasm) => Some(format!("{}/wire", host.package)),
        (root_mode, dependency_mode) if root_mode == dependency_mode => Some(host.package.clone()),
        _ => None,
    }
}

fn typescript_declaration_imports(
    contract: &ResolvedContract,
    mode: TypeScriptMode,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
) -> String {
    let mut output = String::new();
    for dependency in contract.dependencies.values() {
        if !dependency_is_emitted(&dependency.owner, emitted_types) {
            continue;
        }
        let Some(specifier) = dependency
            .typescript
            .as_ref()
            .and_then(|host| typescript_dependency_specifier(host, mode))
        else {
            continue;
        };
        let type_only = contract
            .foreign_types
            .iter()
            .filter(|(identity, definition)| {
                identity.owner == dependency.owner
                    && emitted_types.is_none_or(|types| types.contains(*identity))
                    && !matches!(definition.shape, TypeShape::StringEnum { .. })
            })
            .map(|(_, definition)| definition.name.as_str())
            .collect::<Vec<_>>();
        let enums = contract
            .foreign_types
            .iter()
            .filter(|(identity, definition)| {
                identity.owner == dependency.owner
                    && emitted_types.is_none_or(|types| types.contains(*identity))
                    && matches!(definition.shape, TypeShape::StringEnum { .. })
            })
            .map(|(_, definition)| definition.name.as_str())
            .collect::<Vec<_>>();
        let errors = contract
            .foreign_errors
            .iter()
            .filter(|(identity, _)| identity.owner == dependency.owner)
            .map(|(_, definition)| definition.name.as_str())
            .collect::<Vec<_>>();
        if !type_only.is_empty() {
            output.push_str(&format!(
                "import type {{ {} }} from {};\nexport type {{ {} }} from {};\n",
                type_only.join(", "),
                json_string(&specifier),
                type_only.join(", "),
                json_string(&specifier)
            ));
        }
        if !enums.is_empty() {
            output.push_str(&format!(
                "import {{ {} }} from {};\nexport {{ {} }} from {};\n",
                enums.join(", "),
                json_string(&specifier),
                enums.join(", "),
                json_string(&specifier)
            ));
        }
        if mode == TypeScriptMode::Wasm && !errors.is_empty() {
            output.push_str(&format!(
                "import {{ {} }} from {};\nexport {{ {} }} from {};\n",
                errors.join(", "),
                json_string(&specifier),
                errors.join(", "),
                json_string(&specifier)
            ));
        }
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

fn typescript_runtime_imports(
    contract: &ResolvedContract,
    mode: TypeScriptMode,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
) -> String {
    let mut output = String::new();
    for (index, dependency) in contract.dependencies.values().enumerate() {
        if !dependency_is_emitted(&dependency.owner, emitted_types) {
            continue;
        }
        let Some(specifier) = dependency
            .typescript
            .as_ref()
            .and_then(|host| typescript_dependency_specifier(host, mode))
        else {
            continue;
        };
        output.push_str(&format!(
            "import {{ CONTRACT_FINGERPRINT as __rspyts_dependency_{index} }} from {};\n",
            json_string(&specifier)
        ));
        let errors = contract
            .foreign_errors
            .iter()
            .filter(|(identity, _)| identity.owner == dependency.owner)
            .map(|(_, definition)| definition.name.as_str())
            .collect::<Vec<_>>();
        if mode == TypeScriptMode::Wasm && !errors.is_empty() {
            output.push_str(&format!(
                "import {{ {} }} from {};\nexport {{ {} }};\n",
                errors.join(", "),
                json_string(&specifier),
                errors.join(", ")
            ));
        }
        let enums = contract
            .foreign_types
            .iter()
            .filter(|(identity, definition)| {
                identity.owner == dependency.owner
                    && emitted_types.is_none_or(|types| types.contains(*identity))
                    && matches!(definition.shape, TypeShape::StringEnum { .. })
            })
            .map(|(_, definition)| definition.name.as_str())
            .collect::<Vec<_>>();
        if !enums.is_empty() {
            output.push_str(&format!(
                "export {{ {} }} from {};\n",
                enums.join(", "),
                json_string(&specifier)
            ));
        }
    }
    output
}

fn dependency_fingerprint_assertions(
    contract: &ResolvedContract,
    mode: TypeScriptMode,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
) -> String {
    let mut output = String::new();
    for (index, dependency) in contract.dependencies.values().enumerate() {
        if !dependency_is_emitted(&dependency.owner, emitted_types) {
            continue;
        }
        let Some(host) = dependency
            .typescript
            .as_ref()
            .filter(|host| typescript_dependency_specifier(host, mode).is_some())
        else {
            continue;
        };
        let package = &host.package;
        output.push_str(&format!(
            "if (__rspyts_dependency_{index} !== {}) {{\n  throw new globalThis.Error({} + globalThis.String(__rspyts_dependency_{index}));\n}}\n",
            json_string(&dependency.fingerprint),
            json_string(&format!(
                "rspyts dependency `{package}` fingerprint mismatch: expected {}, received ",
                dependency.fingerprint
            ))
        ));
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output
}

fn dependency_is_emitted(
    owner: &rspyts::ir::CargoPackageId,
    emitted_types: Option<&BTreeSet<DefinitionId>>,
) -> bool {
    emitted_types.is_none_or(|types| types.iter().any(|identity| identity.owner == *owner))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rspyts::ir::*;

    use super::*;

    fn resolved(manifest: Manifest, mode: TypeScriptMode) -> ResolvedContract {
        ResolvedContract {
            manifest,
            hosts: crate::LockedHosts {
                python: None,
                typescript: Some(crate::LockedTypeScriptHost {
                    package: "sample".into(),
                    mode,
                }),
            },
            dependencies: BTreeMap::new(),
            foreign_types: BTreeMap::new(),
            foreign_errors: BTreeMap::new(),
        }
    }

    #[test]
    fn wasm_packages_export_a_canonical_wire_subpath_without_false_side_effect_metadata() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.2.3".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![
                TypeDef {
                    owner: owner.clone(),
                    id: "sample::Status".into(),
                    name: "Status".into(),
                    docs: None,
                    shape: TypeShape::StringEnum {
                        variants: vec![EnumVariantDef {
                            rust_name: "Ready".into(),
                            wire_name: "ready".into(),
                            docs: None,
                            fields: vec![],
                        }],
                    },
                },
                TypeDef {
                    owner: owner.clone(),
                    id: "sample::BigValue".into(),
                    name: "BigValue".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![FieldDef {
                            rust_name: "value".into(),
                            wire_name: "value".into(),
                            docs: None,
                            ty: TypeRef::Int {
                                signed: false,
                                bits: 64,
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        }],
                    },
                },
            ],
            errors: vec![],
            functions: vec![FunctionDef {
                owner,
                rust_name: "run".into(),
                host_name: "run".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::BigValue"),
                },
                error: None,
            }],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-wire-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        emit(
            &root,
            &TypeScriptConfig {
                package: "@example/sample".into(),
                mode: TypeScriptMode::Wasm,
            },
            &contract,
            "sha256:sample",
        )
        .unwrap();

        let package: Value =
            serde_json::from_slice(&fs::read(root.join("typescript/package.json")).unwrap())
                .unwrap();
        assert!(package.get("sideEffects").is_none());
        assert_eq!(
            package["exports"]["./wire"]["types"],
            Value::String("./wire.d.ts".into())
        );
        assert_eq!(
            package["exports"]["./wire"]["import"],
            Value::String("./wire.js".into())
        );
        assert_eq!(
            package["files"],
            serde_json::json!([
                "index.js",
                "index.d.ts",
                "contract.json",
                "native.js",
                "native_bg.wasm",
                "wire.js",
                "wire.d.ts"
            ])
        );

        let wire_declarations = fs::read_to_string(root.join("typescript/wire.d.ts")).unwrap();
        assert!(wire_declarations.contains("export enum Status"));
        assert!(!wire_declarations.contains("BigValue"));
        assert!(!wire_declarations.contains("function run"));
        assert!(!wire_declarations.contains("InitInput"));
        let main_declarations = fs::read_to_string(root.join("typescript/index.d.ts")).unwrap();
        assert!(main_declarations.contains("export interface BigValue"));
        assert!(main_declarations.contains("readonly value: bigint;"));
        assert!(main_declarations.contains("run(): BigValue"));
        let wire_runtime = fs::read_to_string(root.join("typescript/wire.js")).unwrap();
        assert!(wire_runtime.contains("export const CONTRACT_FINGERPRINT = \"sha256:sample\";"));
        assert!(wire_runtime.contains("export const Status = globalThis.Object.freeze"));
        assert!(!wire_runtime.contains("initializeNative"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_packages_list_only_emitted_files() {
        let contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "sample".into(),
                crate_version: "1.2.3".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Static,
        );
        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-static-package-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        emit(
            &root,
            &TypeScriptConfig {
                package: "@example/sample".into(),
                mode: TypeScriptMode::Static,
            },
            &contract,
            "sha256:sample",
        )
        .unwrap();

        let package: Value =
            serde_json::from_slice(&fs::read(root.join("typescript/package.json")).unwrap())
                .unwrap();
        assert_eq!(
            package["files"],
            serde_json::json!(["index.js", "index.d.ts", "contract.json"])
        );
        assert_eq!(
            fs::read_dir(root.join("typescript"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "contract.json".to_owned(),
                "index.d.ts".to_owned(),
                "index.js".to_owned(),
                "package.json".to_owned(),
            ])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn emits_exact_json_and_wide_integer_types() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![FunctionDef {
                owner,
                rust_name: "read".into(),
                host_name: "read".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![ParamDef {
                    rust_name: "value".into(),
                    host_name: "value".into(),
                    ty: TypeRef::Json,
                }],
                returns: TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
                error: None,
            }],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let generated = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Wasm,
        );
        assert!(
            generated.contains(
                "type JsonValue = null | boolean | number | string | readonly JsonValue[]"
            )
        );
        assert!(generated.contains("read(value: JsonValue): bigint"));
        assert!(!generated.contains("unknown"));
    }

    #[test]
    fn static_wide_integers_are_safe_json_numbers_while_wasm_uses_bigint() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![TypeDef {
                owner: owner.clone(),
                id: "sample::WideValue".into(),
                name: "WideValue".into(),
                docs: None,
                shape: TypeShape::Struct {
                    fields: vec![
                        FieldDef {
                            rust_name: "value".into(),
                            wire_name: "value".into(),
                            docs: None,
                            ty: TypeRef::Int {
                                signed: true,
                                bits: 64,
                            },
                            required: true,
                            default: None,
                            constraints: FieldConstraints {
                                literal: Some(ScalarValue::I64(42)),
                                ..Default::default()
                            },
                        },
                        FieldDef {
                            rust_name: "marker".into(),
                            wire_name: "marker".into(),
                            docs: None,
                            ty: TypeRef::Unit,
                            required: true,
                            default: None,
                            constraints: FieldConstraints::default(),
                        },
                    ],
                },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![ConstantDef {
                owner,
                rust_name: "LARGE_COUNTER".into(),
                host_name: "LARGE_COUNTER".into(),
                docs: None,
                target: Target::Both,
                ty: TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
                value: serde_json::json!(4_294_967_296_u64),
            }],
        };
        let static_contract = resolved(manifest.clone(), TypeScriptMode::Static);
        validate_static_contract(&static_contract).unwrap();
        let static_declarations = declarations(
            &static_contract,
            "sha256:test",
            &type_names(&static_contract),
            TypeScriptMode::Static,
        );
        let static_javascript = static_runtime(&static_contract, "sha256:test");
        assert!(static_declarations.contains("readonly value: 42;"));
        assert!(static_declarations.contains("readonly marker: null;"));
        assert!(static_declarations.contains("export const LARGE_COUNTER: 4294967296;"));
        assert!(static_javascript.contains("__rspyts_deep_freeze(4294967296);"));
        assert!(!static_declarations.contains("bigint"));
        assert!(!static_javascript.contains("4294967296n"));

        let wasm_contract = resolved(manifest, TypeScriptMode::Wasm);
        let wasm_declarations = declarations(
            &wasm_contract,
            "sha256:test",
            &type_names(&wasm_contract),
            TypeScriptMode::Wasm,
        );
        let wasm_javascript = wasm_runtime(&wasm_contract, "sha256:test");
        assert!(wasm_declarations.contains("readonly value: 42n;"));
        assert!(wasm_declarations.contains("readonly marker: void;"));
        assert!(wasm_declarations.contains("export const LARGE_COUNTER: 4294967296n;"));
        assert!(wasm_javascript.contains("__rspyts_deep_freeze(4294967296n);"));
    }

    #[test]
    fn static_constants_reject_wide_integers_that_javascript_cannot_represent() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![ConstantDef {
                owner,
                rust_name: "UNSAFE".into(),
                host_name: "UNSAFE".into(),
                docs: None,
                target: Target::Static,
                ty: TypeRef::Int {
                    signed: false,
                    bits: 64,
                },
                value: serde_json::json!(9_007_199_254_740_992_u64),
            }],
        };
        let contract = resolved(manifest, TypeScriptMode::Static);
        let error = validate_static_contract(&contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outside JavaScript's safe integer range")
        );
    }

    #[test]
    fn wasm_wire_keeps_safe_wide_constants_and_omits_unsafe_values() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![
                ConstantDef {
                    owner: owner.clone(),
                    rust_name: "SAFE_WIDE".into(),
                    host_name: "SAFE_WIDE".into(),
                    docs: None,
                    target: Target::Both,
                    ty: TypeRef::Int {
                        signed: false,
                        bits: 64,
                    },
                    value: serde_json::json!(9_007_199_254_740_991_u64),
                },
                ConstantDef {
                    owner,
                    rust_name: "UNSAFE_WIDE".into(),
                    host_name: "UNSAFE_WIDE".into(),
                    docs: None,
                    target: Target::Both,
                    ty: TypeRef::Int {
                        signed: false,
                        bits: 64,
                    },
                    value: serde_json::json!(9_007_199_254_740_992_u64),
                },
            ],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let names = type_names(&contract);
        let root_declarations =
            declarations(&contract, "sha256:test", &names, TypeScriptMode::Wasm);
        let root_runtime = wasm_runtime(&contract, "sha256:test");
        let wire_constants = wire_constant_indices(&contract);
        let wire_types = wire_type_ids(&contract, &wire_constants);
        let wire_declarations = declarations_with_types(
            &contract,
            "sha256:test",
            &names,
            TypeScriptMode::Static,
            Some(&wire_types),
            Some(&wire_constants),
        );
        let wire_runtime = static_runtime_with_types(
            &contract,
            "sha256:test",
            Some(&wire_types),
            Some(&wire_constants),
        );

        assert!(root_declarations.contains("export const SAFE_WIDE: 9007199254740991n;"));
        assert!(root_declarations.contains("export const UNSAFE_WIDE: 9007199254740992n;"));
        assert!(root_runtime.contains("SAFE_WIDE = __rspyts_deep_freeze(9007199254740991n);"));
        assert!(root_runtime.contains("UNSAFE_WIDE = __rspyts_deep_freeze(9007199254740992n);"));
        assert!(wire_declarations.contains("export const SAFE_WIDE: 9007199254740991;"));
        assert!(wire_runtime.contains("SAFE_WIDE = __rspyts_deep_freeze(9007199254740991);"));
        assert!(!wire_declarations.contains("UNSAFE_WIDE"));
        assert!(!wire_runtime.contains("UNSAFE_WIDE"));
    }

    #[test]
    fn static_types_require_transitive_safe_integer_proofs() {
        let owner = CargoPackageId::new("sample");
        let counter = DefinitionId::new(owner.as_str(), "sample::Counter");
        let envelope = DefinitionId::new(owner.as_str(), "sample::Envelope");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![
                TypeDef {
                    owner: owner.clone(),
                    id: counter.id.clone(),
                    name: "Counter".into(),
                    docs: None,
                    shape: TypeShape::Alias {
                        target: TypeRef::Int {
                            signed: false,
                            bits: 64,
                        },
                    },
                },
                TypeDef {
                    owner,
                    id: envelope.id,
                    name: "Envelope".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![FieldDef {
                            rust_name: "values".into(),
                            wire_name: "values".into(),
                            docs: None,
                            ty: TypeRef::List {
                                item: Box::new(TypeRef::Named { identity: counter }),
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        }],
                    },
                },
            ],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Static);
        let error = validate_static_contract(&contract).unwrap_err();
        assert!(error.to_string().contains("exposes u64"));
        assert!(error.to_string().contains("bounded safe literal"));

        let mut buffer_contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "sample".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Static,
        );
        buffer_contract.manifest.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Samples".into(),
            name: "Samples".into(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::Buffer {
                    element: BufferElement::I64,
                },
            },
        });
        let error = validate_static_contract(&buffer_contract).unwrap_err();
        assert!(error.to_string().contains("i64 buffer"));
        assert!(error.to_string().contains("per-item bounds"));
    }

    #[test]
    fn static_dependency_reachability_ignores_python_only_unsafe_types() {
        let root_owner = CargoPackageId::new("root");
        let dependency_owner = CargoPackageId::new("dependency");
        let big_identity = DefinitionId::new(dependency_owner.as_str(), "dependency::BigValue");
        let big_definition = TypeDef {
            owner: dependency_owner.clone(),
            id: big_identity.id.clone(),
            name: "BigValue".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "value".into(),
                    wire_name: "value".into(),
                    docs: None,
                    ty: TypeRef::Int {
                        signed: false,
                        bits: 64,
                    },
                    required: true,
                    default: None,
                    constraints: Default::default(),
                }],
            },
        };
        let mut contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "root".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![],
                errors: vec![],
                functions: vec![FunctionDef {
                    owner: root_owner.clone(),
                    rust_name: "inspect".into(),
                    host_name: "inspect".into(),
                    docs: None,
                    target: Target::Python,
                    params: vec![ParamDef {
                        rust_name: "value".into(),
                        host_name: "value".into(),
                        ty: TypeRef::Named {
                            identity: big_identity.clone(),
                        },
                    }],
                    returns: TypeRef::Unit,
                    error: None,
                }],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Static,
        );
        contract.dependencies.insert(
            "dependency".into(),
            crate::LockedDependency {
                owner: dependency_owner,
                crate_version: "2.0.0".into(),
                fingerprint: "sha256:dependency".into(),
                python: Some("dependency".into()),
                typescript: Some(crate::LockedTypeScriptHost {
                    package: "@example/dependency".into(),
                    mode: TypeScriptMode::Wasm,
                }),
                types: vec![big_definition.clone()],
                errors: vec![],
            },
        );
        contract
            .foreign_types
            .insert(big_identity.clone(), big_definition);

        validate_static_contract(&contract).unwrap();
        let generated = declarations(
            &contract,
            "sha256:root",
            &type_names(&contract),
            TypeScriptMode::Static,
        );
        let runtime = static_runtime(&contract, "sha256:root");
        assert!(!generated.contains("@example/dependency"));
        assert!(!generated.contains("BigValue"));
        assert!(!runtime.contains("@example/dependency"));
        assert!(typescript_peer_dependencies(&contract, TypeScriptMode::Static).is_empty());

        contract.manifest.types.push(TypeDef {
            owner: root_owner,
            id: "root::Envelope".into(),
            name: "Envelope".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "value".into(),
                    wire_name: "value".into(),
                    docs: None,
                    ty: TypeRef::Named {
                        identity: big_identity,
                    },
                    required: true,
                    default: None,
                    constraints: Default::default(),
                }],
            },
        });
        let error = validate_static_contract(&contract).unwrap_err();
        assert!(error.to_string().contains("Envelope.value"));
        assert!(error.to_string().contains("exposes u64"));
    }

    #[test]
    fn static_validation_checks_recursive_shapes_ranges_and_constraints() {
        let owner = CargoPackageId::new("sample");
        let status = DefinitionId::new(owner.as_str(), "sample::Status");
        let payload = DefinitionId::new(owner.as_str(), "sample::Payload");
        let selection = DefinitionId::new(owner.as_str(), "sample::Selection");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![
                TypeDef {
                    owner: owner.clone(),
                    id: status.id.clone(),
                    name: "Status".into(),
                    docs: None,
                    shape: TypeShape::StringEnum {
                        variants: vec![EnumVariantDef {
                            rust_name: "Ready".into(),
                            wire_name: "ready".into(),
                            docs: None,
                            fields: vec![],
                        }],
                    },
                },
                TypeDef {
                    owner: owner.clone(),
                    id: payload.id.clone(),
                    name: "Payload".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![
                            FieldDef {
                                rust_name: "count".into(),
                                wire_name: "count".into(),
                                docs: None,
                                ty: TypeRef::Int {
                                    signed: false,
                                    bits: 8,
                                },
                                required: true,
                                default: None,
                                constraints: FieldConstraints {
                                    ge: Some(1),
                                    ..Default::default()
                                },
                            },
                            FieldDef {
                                rust_name: "status".into(),
                                wire_name: "status".into(),
                                docs: None,
                                ty: TypeRef::Named {
                                    identity: status.clone(),
                                },
                                required: true,
                                default: None,
                                constraints: Default::default(),
                            },
                        ],
                    },
                },
                TypeDef {
                    owner: owner.clone(),
                    id: selection.id.clone(),
                    name: "Selection".into(),
                    docs: None,
                    shape: TypeShape::TaggedEnum {
                        tag: "kind".into(),
                        variants: vec![EnumVariantDef {
                            rust_name: "Included".into(),
                            wire_name: "included".into(),
                            docs: None,
                            fields: vec![FieldDef {
                                rust_name: "payload".into(),
                                wire_name: "payload".into(),
                                docs: None,
                                ty: TypeRef::Named {
                                    identity: payload.clone(),
                                },
                                required: true,
                                default: None,
                                constraints: Default::default(),
                            }],
                        }],
                    },
                },
            ],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Static);
        let selection_ref = TypeRef::Named {
            identity: selection,
        };
        validate_static_value(
            &serde_json::json!({
                "kind": "included",
                "payload": {"count": 1, "status": "ready"}
            }),
            &selection_ref,
            &contract,
            "constant `SELECTION`",
        )
        .unwrap();

        for (value, expected) in [
            (
                serde_json::json!({"kind": "included"}),
                "expected required field",
            ),
            (
                serde_json::json!({
                    "kind": "included",
                    "payload": {"count": 1, "status": "ready", "extra": true}
                }),
                "unknown field",
            ),
            (
                serde_json::json!({
                    "kind": "unknown",
                    "payload": {"count": 1, "status": "ready"}
                }),
                "declared tagged variant",
            ),
            (
                serde_json::json!({
                    "kind": "included",
                    "payload": {"count": 256, "status": "ready"}
                }),
                "unsigned 8-bit integer",
            ),
            (
                serde_json::json!({
                    "kind": "included",
                    "payload": {"count": 0, "status": "ready"}
                }),
                "integer >= 1",
            ),
            (
                serde_json::json!({
                    "kind": "included",
                    "payload": {"count": 1, "status": "waiting"}
                }),
                "declared string enum variant",
            ),
        ] {
            let error =
                validate_static_value(&value, &selection_ref, &contract, "constant `SELECTION`")
                    .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }

        for (value, ty, expected) in [
            (
                serde_json::json!([1]),
                TypeRef::Tuple {
                    items: vec![TypeRef::String, TypeRef::Bool],
                },
                "tuple with 2 elements",
            ),
            (
                serde_json::json!([256]),
                TypeRef::Bytes,
                "unsigned 8-bit integer",
            ),
            (
                serde_json::json!([-1]),
                TypeRef::Buffer {
                    element: BufferElement::U16,
                },
                "unsigned 16-bit integer",
            ),
            (
                serde_json::json!([1.0e40]),
                TypeRef::Buffer {
                    element: BufferElement::F32,
                },
                "finite f32",
            ),
            (
                Value::from(f64::NAN),
                TypeRef::Float { bits: 64 },
                "finite f64",
            ),
            (
                serde_json::json!({"nested": [9_007_199_254_740_992_u64]}),
                TypeRef::Json,
                "outside JavaScript's safe integer range",
            ),
            (
                serde_json::Value::Number(
                    serde_json::Number::from_f64(9_007_199_254_740_992.0).unwrap(),
                ),
                TypeRef::Json,
                "outside JavaScript's safe integer range",
            ),
        ] {
            let error =
                validate_static_value(&value, &ty, &contract, "constant `VALUE`").unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }
    }

    #[test]
    fn static_constraints_skip_null_through_option_aliases() {
        let owner = CargoPackageId::new("sample");
        let optional = DefinitionId::new(owner.as_str(), "sample::OptionalLabel");
        let holder = DefinitionId::new(owner.as_str(), "sample::Holder");
        let contract = resolved(
            Manifest {
                ir_version: rspyts::ir::IR_VERSION,
                crate_name: "sample".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![
                    TypeDef {
                        owner: owner.clone(),
                        id: optional.id.clone(),
                        name: "OptionalLabel".into(),
                        docs: None,
                        shape: TypeShape::Alias {
                            target: TypeRef::Option {
                                item: Box::new(TypeRef::String),
                            },
                        },
                    },
                    TypeDef {
                        owner: owner.clone(),
                        id: holder.id.clone(),
                        name: "Holder".into(),
                        docs: None,
                        shape: TypeShape::Struct {
                            fields: vec![FieldDef {
                                rust_name: "value".into(),
                                wire_name: "value".into(),
                                docs: None,
                                ty: TypeRef::Named { identity: optional },
                                required: true,
                                default: None,
                                constraints: FieldConstraints {
                                    literal: Some(ScalarValue::String("ready".into())),
                                    min_length: Some(3),
                                    ..Default::default()
                                },
                            }],
                        },
                    },
                ],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![ConstantDef {
                    owner,
                    rust_name: "HOLDER".into(),
                    host_name: "HOLDER".into(),
                    docs: None,
                    target: Target::Static,
                    ty: TypeRef::Named { identity: holder },
                    value: serde_json::json!({"value": null}),
                }],
            },
            TypeScriptMode::Static,
        );

        validate_static_contract(&contract).unwrap();
        let generated = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Static,
        );
        assert!(generated.contains("readonly value: \"ready\" | null;"));
    }

    #[test]
    fn static_mode_rejects_executable_typescript_exports() {
        let owner = CargoPackageId::new("sample");
        let mut manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![FunctionDef {
                owner: owner.clone(),
                rust_name: "run".into(),
                host_name: "run".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![],
                returns: TypeRef::Unit,
                error: None,
            }],
            resources: vec![],
            constants: vec![],
        };
        let error = validate_static_contract(&resolved(manifest.clone(), TypeScriptMode::Static))
            .unwrap_err();
        assert!(error.to_string().contains("executable function `run`"));

        manifest.functions.clear();
        manifest.resources.push(ResourceDef {
            owner,
            id: "sample::Runner".into(),
            name: "Runner".into(),
            docs: None,
            target: Target::Both,
            constructors: vec![],
            methods: vec![],
        });
        let error =
            validate_static_contract(&resolved(manifest, TypeScriptMode::Static)).unwrap_err();
        assert!(error.to_string().contains("executable resource `Runner`"));
    }

    #[test]
    fn emits_datetime_literals_defaults_and_readonly_fields() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![TypeDef {
                owner,
                id: "sample::Batch".into(),
                name: "Batch".into(),
                docs: None,
                shape: TypeShape::Struct {
                    fields: vec![
                        FieldDef {
                            rust_name: "revision".into(),
                            wire_name: "revision".into(),
                            docs: None,
                            ty: TypeRef::Int {
                                signed: false,
                                bits: 32,
                            },
                            required: true,
                            default: None,
                            constraints: FieldConstraints {
                                literal: Some(ScalarValue::I64(2)),
                                ..Default::default()
                            },
                        },
                        FieldDef {
                            rust_name: "updated_at".into(),
                            wire_name: "updatedAt".into(),
                            docs: None,
                            ty: TypeRef::DateTime,
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                        FieldDef {
                            rust_name: "count".into(),
                            wire_name: "count".into(),
                            docs: None,
                            ty: TypeRef::Int {
                                signed: false,
                                bits: 32,
                            },
                            required: false,
                            default: Some(ScalarValue::I64(1)),
                            constraints: FieldConstraints {
                                ge: Some(1),
                                ..Default::default()
                            },
                        },
                    ],
                },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Static);
        let generated = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Static,
        );

        assert!(generated.contains("readonly revision: 2;"));
        assert!(generated.contains("readonly updatedAt: string;"));
        assert!(generated.contains("readonly count?: number;"));
    }

    #[test]
    fn declarations_are_deeply_readonly() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![TypeDef {
                owner,
                id: "sample::Nested".into(),
                name: "Nested".into(),
                docs: None,
                shape: TypeShape::Struct {
                    fields: vec![
                        FieldDef {
                            rust_name: "items".into(),
                            wire_name: "items".into(),
                            docs: None,
                            ty: TypeRef::List {
                                item: Box::new(TypeRef::Map {
                                    value: Box::new(TypeRef::Tuple {
                                        items: vec![TypeRef::String, TypeRef::Json],
                                    }),
                                }),
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                        FieldDef {
                            rust_name: "bytes".into(),
                            wire_name: "bytes".into(),
                            docs: None,
                            ty: TypeRef::Bytes,
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                        FieldDef {
                            rust_name: "samples".into(),
                            wire_name: "samples".into(),
                            docs: None,
                            ty: TypeRef::Buffer {
                                element: BufferElement::F64,
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                    ],
                },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest.clone(), TypeScriptMode::Static);
        let generated = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Static,
        );

        assert!(generated.contains(
            "readonly items: readonly ({ readonly [key: string]: readonly [string, JsonValue] })[];"
        ));
        assert!(generated.contains("readonly bytes: readonly number[];"));
        assert!(generated.contains("readonly samples: readonly number[];"));
        assert!(!generated.contains("ReadonlyArray<"));
        assert!(!generated.contains("Record<"));
        assert!(generated.contains("readonly JsonValue[]"));
        assert!(generated.contains("{ readonly [key: string]: JsonValue }"));

        let wasm_contract = resolved(manifest, TypeScriptMode::Wasm);
        let wasm = declarations(
            &wasm_contract,
            "sha256:test",
            &type_names(&wasm_contract),
            TypeScriptMode::Wasm,
        );
        assert!(wasm.contains("readonly bytes: globalThis.Uint8Array;"));
        assert!(wasm.contains("readonly samples: globalThis.Float64Array;"));
    }

    #[test]
    fn declarations_do_not_let_contract_names_shadow_typescript_builtins() {
        if !Command::new("tsc")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return;
        }
        let owner = CargoPackageId::new("sample");
        let record = DefinitionId::new(owner.as_str(), "sample::Record");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![
                TypeDef {
                    owner: owner.clone(),
                    id: record.id.clone(),
                    name: "Record".into(),
                    docs: None,
                    shape: TypeShape::Struct { fields: vec![] },
                },
                TypeDef {
                    owner: owner.clone(),
                    id: "sample::ReadonlyArray".into(),
                    name: "ReadonlyArray".into(),
                    docs: None,
                    shape: TypeShape::Alias {
                        target: TypeRef::List {
                            item: Box::new(TypeRef::Named {
                                identity: record.clone(),
                            }),
                        },
                    },
                },
                TypeDef {
                    owner: owner.clone(),
                    id: "sample::Uint8Array".into(),
                    name: "Uint8Array".into(),
                    docs: None,
                    shape: TypeShape::Struct { fields: vec![] },
                },
                TypeDef {
                    owner,
                    id: "sample::Container".into(),
                    name: "Container".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![
                            FieldDef {
                                rust_name: "records".into(),
                                wire_name: "records".into(),
                                docs: None,
                                ty: TypeRef::Map {
                                    value: Box::new(TypeRef::Named { identity: record }),
                                },
                                required: true,
                                default: None,
                                constraints: Default::default(),
                            },
                            FieldDef {
                                rust_name: "bytes".into(),
                                wire_name: "bytes".into(),
                                docs: None,
                                ty: TypeRef::Bytes,
                                required: true,
                                default: None,
                                constraints: Default::default(),
                            },
                            FieldDef {
                                rust_name: "samples".into(),
                                wire_name: "samples".into(),
                                docs: None,
                                ty: TypeRef::Buffer {
                                    element: BufferElement::F64,
                                },
                                required: true,
                                default: None,
                                constraints: Default::default(),
                            },
                        ],
                    },
                },
            ],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let generated = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Wasm,
        );
        assert!(!generated.contains("ReadonlyArray<"));
        assert!(!generated.contains("Record<"));
        assert!(generated.contains("readonly bytes: globalThis.Uint8Array;"));
        assert!(generated.contains("readonly samples: globalThis.Float64Array;"));

        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-shadow-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("index.d.ts"), generated).unwrap();
        fs::write(
            root.join("acceptance.mts"),
            "import type { Container, Record as ContractRecord, ReadonlyArray as ContractArray } from './index.js';\n\
             declare const value: Container;\n\
             const record: ContractRecord | undefined = value.records.first;\n\
             const array: ContractArray = record === undefined ? [] : [record];\n\
             const bytes: globalThis.Uint8Array = value.bytes;\n\
             const samples: globalThis.Float64Array = value.samples;\n\
             void [array, bytes, samples];\n",
        )
        .unwrap();
        let result = Command::new("tsc")
            .current_dir(&root)
            .args([
                "--noEmit",
                "--strict",
                "--module",
                "NodeNext",
                "--moduleResolution",
                "NodeNext",
                "--target",
                "ES2022",
                "--lib",
                "ES2022,DOM",
                "--skipLibCheck",
                "false",
                "acceptance.mts",
            ])
            .output()
            .expect("TypeScript is required to verify generated declarations");
        assert!(
            result.status.success(),
            "generated declarations did not typecheck:\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_runtime_emits_and_freezes_json_arrays_for_bytes_and_buffers() {
        if !Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return;
        }
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![
                ConstantDef {
                    owner: owner.clone(),
                    rust_name: "CONFIG".into(),
                    host_name: "CONFIG".into(),
                    docs: None,
                    target: Target::Static,
                    ty: TypeRef::Json,
                    value: serde_json::json!({
                        "nested": {"items": [1, {"ready": true}]},
                        "__proto__": {"polluted": true}
                    }),
                },
                ConstantDef {
                    owner: owner.clone(),
                    rust_name: "BYTES".into(),
                    host_name: "BYTES".into(),
                    docs: None,
                    target: Target::Static,
                    ty: TypeRef::Bytes,
                    value: serde_json::json!([1, 2, 3]),
                },
                ConstantDef {
                    owner,
                    rust_name: "SAMPLES".into(),
                    host_name: "SAMPLES".into(),
                    docs: None,
                    target: Target::Static,
                    ty: TypeRef::Buffer {
                        element: BufferElement::F64,
                    },
                    value: serde_json::json!([1.5, 2.5]),
                },
            ],
        };
        let contract = resolved(manifest, TypeScriptMode::Static);
        let mut generated = static_runtime(&contract, "sha256:test");
        generated.push_str(
            "\nconst shallow = Object.freeze({ nested: { mutable: true } });\n\
             __rspyts_deep_freeze(shallow);\n\
             if (!Object.isFrozen(CONFIG) || !Object.isFrozen(CONFIG.nested) || \
                 !Object.isFrozen(CONFIG.nested.items) || \
                 !Object.isFrozen(CONFIG.nested.items[1])) throw new Error('not deeply frozen');\n\
             if (!Object.hasOwn(CONFIG, '__proto__')) \
                 throw new Error('__proto__ was not emitted as an own property');\n\
             if (Object.getPrototypeOf(CONFIG) !== Object.prototype || ({}).polluted) \
                 throw new Error('__proto__ changed an object prototype');\n\
             if (!Object.isFrozen(shallow.nested)) throw new Error('shallow parent skipped');\n\
             if (!Array.isArray(BYTES) || !Object.isFrozen(BYTES)) \
                 throw new Error('bytes were not frozen JSON');\n\
             if (!Array.isArray(SAMPLES) || !Object.isFrozen(SAMPLES)) \
                 throw new Error('buffer was not frozen JSON');\n",
        );
        assert!(!generated.contains("Uint8Array"));
        assert!(!generated.contains("Float64Array"));
        assert!(generated.contains("[\"__proto__\"]"));
        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-freeze-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let module = root.join("generated.mjs");
        fs::write(&module, generated).unwrap();
        let result = Command::new("node")
            .arg(&module)
            .output()
            .expect("Node is required to verify generated TypeScript runtime behavior");
        assert!(
            result.status.success(),
            "generated runtime did not deeply freeze values:\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixed_bytes_have_exact_static_and_wasm_host_surfaces() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![TypeDef {
                owner: owner.clone(),
                id: "sample::Packet".into(),
                name: "Packet".into(),
                docs: None,
                shape: TypeShape::Struct {
                    fields: vec![
                        FieldDef {
                            rust_name: "digest".into(),
                            wire_name: "digest".into(),
                            docs: None,
                            ty: TypeRef::FixedBytes { length: 4 },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                        FieldDef {
                            rust_name: "chunks".into(),
                            wire_name: "chunks".into(),
                            docs: None,
                            ty: TypeRef::List {
                                item: Box::new(TypeRef::FixedBytes { length: 2 }),
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        },
                    ],
                },
            }],
            errors: vec![],
            functions: vec![FunctionDef {
                owner: owner.clone(),
                rust_name: "echo_digest".into(),
                host_name: "echoDigest".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![ParamDef {
                    rust_name: "digest".into(),
                    host_name: "digest".into(),
                    ty: TypeRef::FixedBytes { length: 4 },
                }],
                returns: TypeRef::List {
                    item: Box::new(TypeRef::FixedBytes { length: 2 }),
                },
                error: None,
            }],
            resources: vec![],
            constants: vec![ConstantDef {
                owner,
                rust_name: "DIGEST".into(),
                host_name: "DIGEST".into(),
                docs: None,
                target: Target::Both,
                ty: TypeRef::FixedBytes { length: 4 },
                value: serde_json::json!([0, 1, 2, 255]),
            }],
        };

        let wasm_contract = resolved(manifest.clone(), TypeScriptMode::Wasm);
        let wasm = declarations(
            &wasm_contract,
            "sha256:test",
            &type_names(&wasm_contract),
            TypeScriptMode::Wasm,
        );
        assert!(wasm.contains("readonly digest: globalThis.Uint8Array;"));
        assert!(wasm.contains("readonly chunks: readonly (globalThis.Uint8Array)[];"));
        assert!(wasm.contains(
            "echoDigest(digest: globalThis.Uint8Array): readonly (globalThis.Uint8Array)[]"
        ));
        let wire_constants = wire_constant_indices(&wasm_contract);
        let wire_types = wire_type_ids(&wasm_contract, &wire_constants);
        let wire = declarations_with_types(
            &wasm_contract,
            "sha256:test",
            &type_names(&wasm_contract),
            TypeScriptMode::Static,
            Some(&wire_types),
            Some(&wire_constants),
        );
        assert!(wire.contains("readonly digest: readonly number[] & { readonly length: 4 };"));
        assert!(
            wire.contains(
                "readonly chunks: readonly (readonly number[] & { readonly length: 2 })[];"
            )
        );
        assert!(!wire.contains("function echoDigest"));

        let mut static_manifest = manifest;
        static_manifest.functions.clear();
        let static_contract = resolved(static_manifest, TypeScriptMode::Static);
        validate_static_contract(&static_contract).unwrap();
        let static_declarations = declarations(
            &static_contract,
            "sha256:test",
            &type_names(&static_contract),
            TypeScriptMode::Static,
        );
        assert!(
            static_declarations
                .contains("readonly digest: readonly number[] & { readonly length: 4 };")
        );
        assert!(
            static_declarations.contains(
                "readonly chunks: readonly (readonly number[] & { readonly length: 2 })[];"
            )
        );
        assert!(static_declarations.contains("export const DIGEST: readonly [0, 1, 2, 255];"));

        let mut invalid_length = static_contract.manifest.clone();
        invalid_length.constants[0].value = serde_json::json!([0, 1, 2]);
        let error = validate_static_contract(&resolved(invalid_length, TypeScriptMode::Static))
            .unwrap_err();
        assert!(error.to_string().contains("expected exactly 4 bytes"));

        let mut invalid_item = static_contract.manifest.clone();
        invalid_item.constants[0].value = serde_json::json!([0, 1, 2, 256]);
        let error =
            validate_static_contract(&resolved(invalid_item, TypeScriptMode::Static)).unwrap_err();
        assert!(error.to_string().contains("unsigned 8-bit integer"));

        if Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            let root = std::env::temp_dir().join(format!(
                "rspyts-typescript-fixed-bytes-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join("index.js"),
                static_runtime(&static_contract, "sha256:test"),
            )
            .unwrap();
            fs::write(
                root.join("acceptance.mjs"),
                "import { DIGEST } from './index.js';\n\
                 if (!Array.isArray(DIGEST) || DIGEST.length !== 4 || !Object.isFrozen(DIGEST)) \
                   throw new Error('fixed bytes were not emitted as frozen JSON');\n",
            )
            .unwrap();
            let result = Command::new("node")
                .arg(root.join("acceptance.mjs"))
                .output()
                .expect("Node is required to verify generated fixed bytes");
            assert!(
                result.status.success(),
                "generated fixed bytes failed Node validation:\n{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            fs::remove_dir_all(root).unwrap();
        }

        if Command::new("tsc")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            let root = std::env::temp_dir().join(format!(
                "rspyts-typescript-fixed-bytes-types-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let static_root = root.join("static");
            let wasm_root = root.join("wasm");
            fs::create_dir_all(&static_root).unwrap();
            fs::create_dir_all(&wasm_root).unwrap();
            fs::write(static_root.join("index.d.ts"), &static_declarations).unwrap();
            fs::write(
                static_root.join("acceptance.mts"),
                "import type { Packet } from './index.js';\n\
                 const valid: Packet = { digest: [0, 1, 2, 3] as const, chunks: [[0, 1] as const] };\n\
                 // @ts-expect-error fixed bytes reject a short tuple\n\
                 const invalid: Packet = { digest: [0, 1, 2] as const, chunks: [] };\n\
                 void [valid, invalid];\n",
            )
            .unwrap();
            fs::write(wasm_root.join("index.d.ts"), &wasm).unwrap();
            fs::write(
                wasm_root.join("acceptance.mts"),
                "import { echoDigest } from './index.js';\n\
                 const result = echoDigest(new Uint8Array(4));\n\
                 const first: Uint8Array | undefined = result[0];\n\
                 void first;\n",
            )
            .unwrap();
            for directory in [&static_root, &wasm_root] {
                let result = Command::new("tsc")
                    .current_dir(directory)
                    .args([
                        "--noEmit",
                        "--strict",
                        "--module",
                        "NodeNext",
                        "--moduleResolution",
                        "NodeNext",
                        "--target",
                        "ES2022",
                        "--lib",
                        "ES2022,DOM",
                        "--skipLibCheck",
                        "false",
                        "acceptance.mts",
                    ])
                    .output()
                    .expect("TypeScript is required to verify fixed-bytes declarations");
                assert!(
                    result.status.success(),
                    "generated fixed-bytes declarations did not typecheck in {}:\n{}{}",
                    directory.display(),
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn constants_are_target_filtered_and_enum_typed_in_both_modes() {
        let owner = CargoPackageId::new("sample");
        let status_identity = DefinitionId::new(owner.as_str(), "sample::Status");
        let constant = |name: &str, target, value: &str| ConstantDef {
            owner: owner.clone(),
            rust_name: name.into(),
            host_name: name.into(),
            docs: None,
            target,
            ty: TypeRef::Named {
                identity: status_identity.clone(),
            },
            value: Value::String(value.into()),
        };
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![TypeDef {
                owner: owner.clone(),
                id: status_identity.id.clone(),
                name: "Status".into(),
                docs: None,
                shape: TypeShape::StringEnum {
                    variants: vec![EnumVariantDef {
                        rust_name: "Ready".into(),
                        wire_name: "ready".into(),
                        docs: None,
                        fields: vec![],
                    }],
                },
            }],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![
                constant("TYPESCRIPT_STATUS", Target::Typescript, "ready"),
                constant("SHARED_STATUS", Target::Both, "ready"),
                constant("STATIC_STATUS", Target::Static, "ready"),
                constant("PYTHON_STATUS", Target::Python, "ready"),
            ],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let declarations = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Wasm,
        );
        let wasm = wasm_runtime(&contract, "sha256:test");
        let static_js = static_runtime(&contract, "sha256:test");

        assert!(declarations.contains("export const TYPESCRIPT_STATUS: Status.Ready;"));
        assert!(declarations.contains("export const SHARED_STATUS: Status.Ready;"));
        assert!(!declarations.contains("STATIC_STATUS"));
        assert!(!declarations.contains("PYTHON_STATUS"));
        assert!(wasm.contains("export const TYPESCRIPT_STATUS = __rspyts_deep_freeze(\"ready\");"));
        assert!(wasm.contains("export const SHARED_STATUS = __rspyts_deep_freeze(\"ready\");"));
        assert!(!wasm.contains("STATIC_STATUS"));
        assert!(!wasm.contains("PYTHON_STATUS"));
        assert!(
            static_js.contains("export const TYPESCRIPT_STATUS = __rspyts_deep_freeze(\"ready\");")
        );
        assert!(
            static_js.contains("export const SHARED_STATUS = __rspyts_deep_freeze(\"ready\");")
        );
        assert!(
            static_js.contains("export const STATIC_STATUS = __rspyts_deep_freeze(\"ready\");")
        );
        assert!(!static_js.contains("PYTHON_STATUS"));
    }

    #[test]
    fn dependencies_are_exactly_pinned_and_fingerprints_are_enforced_at_runtime() {
        if !Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return;
        }
        let mut contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "sample".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Static,
        );
        let dependency_owner = CargoPackageId::new("dependency-rust");
        let value_identity =
            DefinitionId::new(dependency_owner.as_str(), "dependency::DependencyValue");
        let value_definition = TypeDef {
            owner: dependency_owner.clone(),
            id: value_identity.id.clone(),
            name: "DependencyValue".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "id".into(),
                    wire_name: "id".into(),
                    docs: None,
                    ty: TypeRef::String,
                    required: true,
                    default: None,
                    constraints: Default::default(),
                }],
            },
        };
        contract.manifest.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::RootValue".into(),
            name: "RootValue".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "dependency".into(),
                    wire_name: "dependency".into(),
                    docs: None,
                    ty: TypeRef::Named {
                        identity: value_identity.clone(),
                    },
                    required: true,
                    default: None,
                    constraints: Default::default(),
                }],
            },
        });
        contract.dependencies.insert(
            "dependency".into(),
            crate::LockedDependency {
                owner: dependency_owner.clone(),
                crate_version: "2.3.4".into(),
                fingerprint: "sha256:dependency".into(),
                python: None,
                typescript: Some(crate::LockedTypeScriptHost {
                    package: "dependency".into(),
                    mode: TypeScriptMode::Static,
                }),
                types: vec![value_definition.clone()],
                errors: vec![],
            },
        );
        contract
            .foreign_types
            .insert(value_identity, value_definition);
        let error_identity =
            DefinitionId::new(dependency_owner.as_str(), "dependency::DependencyError");
        contract.foreign_errors.insert(
            error_identity,
            ErrorDef {
                owner: dependency_owner.clone(),
                id: "dependency::DependencyError".into(),
                name: "DependencyError".into(),
                docs: None,
                variants: vec![],
            },
        );

        assert_eq!(
            typescript_peer_dependencies(&contract, TypeScriptMode::Static),
            BTreeMap::from([("dependency".into(), "2.3.4".into())])
        );
        let declarations = declarations(
            &contract,
            "sha256:root",
            &type_names(&contract),
            TypeScriptMode::Static,
        );
        assert!(!declarations.contains("DependencyError"));
        let runtime = static_runtime(&contract, "sha256:root");
        assert!(runtime.contains(
            "import { CONTRACT_FINGERPRINT as __rspyts_dependency_0 } from \"dependency\";"
        ));
        assert!(runtime.contains("__rspyts_dependency_0 !== \"sha256:dependency\""));
        assert!(!runtime.contains("import { DependencyError }"));

        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-fingerprint-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dependency = root.join("node_modules/dependency");
        fs::create_dir_all(&dependency).unwrap();
        fs::write(
            dependency.join("package.json"),
            "{\"name\":\"dependency\",\"type\":\"module\",\"exports\":\"./index.js\"}\n",
        )
        .unwrap();
        fs::write(
            dependency.join("index.js"),
            "export const CONTRACT_FINGERPRINT = 'sha256:dependency';\n",
        )
        .unwrap();
        fs::write(root.join("package.json"), "{\"type\":\"module\"}\n").unwrap();
        fs::write(root.join("index.js"), &runtime).unwrap();
        fs::write(root.join("acceptance.js"), "await import('./index.js');\n").unwrap();
        let matching = Command::new("node")
            .arg(root.join("acceptance.js"))
            .output()
            .expect("Node is required to verify dependency fingerprints");
        assert!(
            matching.status.success(),
            "matching dependency fingerprint failed:\n{}{}",
            String::from_utf8_lossy(&matching.stdout),
            String::from_utf8_lossy(&matching.stderr)
        );

        fs::write(
            dependency.join("index.js"),
            "export const CONTRACT_FINGERPRINT = 'sha256:wrong';\n",
        )
        .unwrap();
        let mismatched = Command::new("node")
            .arg(root.join("acceptance.js"))
            .output()
            .expect("Node is required to verify dependency fingerprints");
        assert!(!mismatched.status.success());
        assert!(
            String::from_utf8_lossy(&mismatched.stderr).contains(
                "rspyts dependency `dependency` fingerprint mismatch: expected sha256:dependency, received sha256:wrong"
            )
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_roots_import_the_canonical_wasm_wire_surface() {
        let dependency_owner = CargoPackageId::new("owner-rust");
        let status = DefinitionId::new(dependency_owner.as_str(), "owner::Status");
        let record = DefinitionId::new(dependency_owner.as_str(), "owner::Record");
        let array = DefinitionId::new(dependency_owner.as_str(), "owner::ReadonlyArray");
        let status_definition = TypeDef {
            owner: dependency_owner.clone(),
            id: status.id.clone(),
            name: "Status".into(),
            docs: None,
            shape: TypeShape::StringEnum {
                variants: vec![EnumVariantDef {
                    rust_name: "Ready".into(),
                    wire_name: "ready".into(),
                    docs: None,
                    fields: vec![],
                }],
            },
        };
        let record_definition = TypeDef {
            owner: dependency_owner.clone(),
            id: record.id.clone(),
            name: "Record".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![
                    FieldDef {
                        rust_name: "revision".into(),
                        wire_name: "revision".into(),
                        docs: None,
                        ty: TypeRef::Int {
                            signed: false,
                            bits: 32,
                        },
                        required: true,
                        default: None,
                        constraints: Default::default(),
                    },
                    FieldDef {
                        rust_name: "marker".into(),
                        wire_name: "marker".into(),
                        docs: None,
                        ty: TypeRef::Unit,
                        required: true,
                        default: None,
                        constraints: Default::default(),
                    },
                    FieldDef {
                        rust_name: "status".into(),
                        wire_name: "status".into(),
                        docs: None,
                        ty: TypeRef::Named {
                            identity: status.clone(),
                        },
                        required: true,
                        default: None,
                        constraints: Default::default(),
                    },
                    FieldDef {
                        rust_name: "metadata".into(),
                        wire_name: "metadata".into(),
                        docs: None,
                        ty: TypeRef::Map {
                            value: Box::new(TypeRef::String),
                        },
                        required: true,
                        default: None,
                        constraints: Default::default(),
                    },
                ],
            },
        };
        let array_definition = TypeDef {
            owner: dependency_owner.clone(),
            id: array.id.clone(),
            name: "ReadonlyArray".into(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::List {
                    item: Box::new(TypeRef::Named {
                        identity: record.clone(),
                    }),
                },
            },
        };
        let root_owner = CargoPackageId::new("consumer-rust");
        let mut contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "consumer-rust".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![TypeDef {
                    owner: root_owner,
                    id: "consumer::Batch".into(),
                    name: "Batch".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![FieldDef {
                            rust_name: "records".into(),
                            wire_name: "records".into(),
                            docs: None,
                            ty: TypeRef::Named {
                                identity: array.clone(),
                            },
                            required: true,
                            default: None,
                            constraints: Default::default(),
                        }],
                    },
                }],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Static,
        );
        contract.dependencies.insert(
            "owner".into(),
            crate::LockedDependency {
                owner: dependency_owner.clone(),
                crate_version: "4.5.6".into(),
                fingerprint: "sha256:owner".into(),
                python: None,
                typescript: Some(crate::LockedTypeScriptHost {
                    package: "@example/owner".into(),
                    mode: TypeScriptMode::Wasm,
                }),
                types: vec![
                    status_definition.clone(),
                    record_definition.clone(),
                    array_definition.clone(),
                ],
                errors: vec![],
            },
        );
        contract.foreign_types.insert(status, status_definition);
        contract.foreign_types.insert(record, record_definition);
        contract.foreign_types.insert(array, array_definition);

        validate_static_contract(&contract).unwrap();
        let generated = declarations(
            &contract,
            "sha256:consumer",
            &type_names(&contract),
            TypeScriptMode::Static,
        );
        assert!(
            generated
                .contains("import type { ReadonlyArray, Record } from \"@example/owner/wire\";")
        );
        assert!(generated.contains("import { Status } from \"@example/owner/wire\";"));
        assert!(generated.contains("readonly records: ReadonlyArray;"));
        assert!(!generated.contains("export enum Status"));
        assert!(!generated.contains("export interface Record"));
        assert!(!generated.contains("export type ReadonlyArray ="));

        let runtime = static_runtime(&contract, "sha256:consumer");
        assert!(runtime.contains(
            "import { CONTRACT_FINGERPRINT as __rspyts_dependency_0 } from \"@example/owner/wire\";"
        ));
        assert!(runtime.contains("export { Status } from \"@example/owner/wire\";"));
        assert!(runtime.contains("__rspyts_dependency_0 !== \"sha256:owner\""));
        assert!(!runtime.contains("export const Status = globalThis.Object.freeze"));
        assert_eq!(
            typescript_peer_dependencies(&contract, TypeScriptMode::Static),
            BTreeMap::from([("@example/owner".into(), "4.5.6".into())])
        );
    }

    #[test]
    fn contract_error_named_error_never_shadows_javascript_error() {
        let owner = CargoPackageId::new("sample");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![ErrorDef {
                owner,
                id: "sample::Error".into(),
                name: "Error".into(),
                docs: None,
                variants: vec![],
            }],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let declarations = declarations(
            &contract,
            "sha256:test",
            &type_names(&contract),
            TypeScriptMode::Wasm,
        );
        let runtime = wasm_runtime(&contract, "sha256:test");

        assert!(declarations.contains("class Error extends globalThis.Error"));
        assert!(runtime.contains("class Error extends globalThis.Error"));
        assert!(runtime.contains("error instanceof globalThis.Error"));
        assert!(runtime.contains("new globalThis.Error("));
        assert!(!runtime.contains("class Error extends Error"));
    }

    #[test]
    fn wasm_runtime_deeply_freezes_function_and_method_results() {
        let owner = CargoPackageId::new("sample");
        let runner_identity = DefinitionId::new(owner.as_str(), "sample::Runner");
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![FunctionDef {
                owner: owner.clone(),
                rust_name: "snapshot".into(),
                host_name: "snapshot".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![],
                returns: TypeRef::Json,
                error: None,
            }],
            resources: vec![ResourceDef {
                owner: owner.clone(),
                id: runner_identity.id.clone(),
                name: "Runner".into(),
                docs: None,
                target: Target::Typescript,
                constructors: vec![FunctionDef {
                    owner,
                    rust_name: "new".into(),
                    host_name: "new".into(),
                    docs: None,
                    target: Target::Typescript,
                    params: vec![],
                    returns: TypeRef::Named {
                        identity: runner_identity,
                    },
                    error: None,
                }],
                methods: vec![MethodDef {
                    rust_name: "snapshot".into(),
                    host_name: "snapshot".into(),
                    docs: None,
                    target: Target::Typescript,
                    mutable: false,
                    params: vec![],
                    returns: TypeRef::Json,
                    error: None,
                }],
            }],
            constants: vec![],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let runtime = wasm_runtime(&contract, "sha256:test");

        assert!(
            runtime.contains(
                "return __rspyts_deep_freeze(__rspyts_native.__rspyts_export_snapshot());"
            )
        );
        assert!(
            runtime.contains(
                "return __rspyts_deep_freeze(this.__rspyts_require_handle().__rspyts_export_snapshot());"
            )
        );
        assert!(runtime.contains("globalThis.ArrayBuffer.isView(__rspyts_value)"));
        assert!(
            runtime.contains(
                "for (const __rspyts_nested of globalThis.Object.values(__rspyts_value))"
            )
        );
    }

    #[test]
    fn packed_runtime_private_names_do_not_capture_contract_bindings() {
        if !["node", "npm"].into_iter().all(|command| {
            Command::new(command)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        }) {
            return;
        }

        let owner = CargoPackageId::new("sample");
        let error_identity = DefinitionId::new(owner.as_str(), "sample::DomainError");
        let runner_identity = DefinitionId::new(owner.as_str(), "sample::Runner");
        let param = |name: &str| ParamDef {
            rust_name: name.into(),
            host_name: name.into(),
            ty: TypeRef::String,
        };
        let method = |name: &str, params: Vec<ParamDef>| MethodDef {
            rust_name: name.into(),
            host_name: name.into(),
            docs: None,
            target: Target::Typescript,
            mutable: false,
            params,
            returns: TypeRef::Json,
            error: None,
        };
        let manifest = Manifest {
            ir_version: 4,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "native".into(),
            imports: vec![],
            types: vec![],
            errors: vec![ErrorDef {
                owner: owner.clone(),
                id: error_identity.id.clone(),
                name: "DomainError".into(),
                docs: None,
                variants: vec![ErrorVariantDef {
                    rust_name: "Failed".into(),
                    code: "failed".into(),
                    docs: None,
                    fields: vec![],
                }],
            }],
            functions: vec![
                FunctionDef {
                    owner: owner.clone(),
                    rust_name: "deep_freeze".into(),
                    host_name: "deepFreeze".into(),
                    docs: None,
                    target: Target::Typescript,
                    params: vec![],
                    returns: TypeRef::String,
                    error: None,
                },
                FunctionDef {
                    owner: owner.clone(),
                    rust_name: "run".into(),
                    host_name: "run".into(),
                    docs: None,
                    target: Target::Typescript,
                    params: ["native", "translateError", "globalThis", "DomainError"]
                        .into_iter()
                        .map(param)
                        .collect(),
                    returns: TypeRef::Json,
                    error: Some(error_identity.clone()),
                },
            ],
            resources: vec![ResourceDef {
                owner: owner.clone(),
                id: runner_identity.id.clone(),
                name: "Runner".into(),
                docs: None,
                target: Target::Typescript,
                constructors: vec![
                    FunctionDef {
                        owner: owner.clone(),
                        rust_name: "new".into(),
                        host_name: "new".into(),
                        docs: None,
                        target: Target::Typescript,
                        params: vec![param("native")],
                        returns: TypeRef::Named {
                            identity: runner_identity.clone(),
                        },
                        error: None,
                    },
                    FunctionDef {
                        owner: owner.clone(),
                        rust_name: "from_value".into(),
                        host_name: "fromValue".into(),
                        docs: None,
                        target: Target::Typescript,
                        params: vec![param("resource"), param("Runner")],
                        returns: TypeRef::Named {
                            identity: runner_identity,
                        },
                        error: None,
                    },
                ],
                methods: vec![
                    method("handle", vec![param("deepFreeze")]),
                    method("requireHandle", vec![]),
                    method("close", vec![]),
                ],
            }],
            constants: vec![ConstantDef {
                owner,
                rust_name: "TRANSLATE_ERROR".into(),
                host_name: "translateError".into(),
                docs: None,
                target: Target::Static,
                ty: TypeRef::String,
                value: serde_json::json!("wire"),
            }],
        };
        let contract = resolved(manifest, TypeScriptMode::Wasm);
        let runtime = wasm_runtime(&contract, "sha256:test");
        assert!(runtime.contains(
            "import __rspyts_initialize_native, * as __rspyts_native from \"./native.js\";"
        ));
        assert!(runtime.contains("function __rspyts_deep_freeze("));
        assert!(runtime.contains("function __rspyts_translate_error("));
        assert!(runtime.contains("const __rspyts_resource ="));
        assert!(runtime.contains("this.__rspyts_require_handle()"));
        assert!(!runtime.contains("function deepFreeze(value"));
        assert!(!runtime.contains("const resource ="));

        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-private-runtime-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        emit(
            &root,
            &TypeScriptConfig {
                package: "sample".into(),
                mode: TypeScriptMode::Wasm,
            },
            &contract,
            "sha256:test",
        )
        .unwrap();
        let package = root.join("typescript");
        fs::write(
            package.join("native.js"),
            "export default function init() { return Promise.resolve({}); }\n\
             export function __rspyts_export_deepFreeze() { return 'public'; }\n\
             export function __rspyts_export_run() { throw { code: 'failed', message: 'boom' }; }\n\
             export class __RspytsWasmRunner {\n\
               constructor(value) { this.value = value; }\n\
               static __rspyts_export_fromValue(resource, Runner) { return new this(`${resource}:${Runner}`); }\n\
               __rspyts_export_handle(deepFreeze) { return { deepFreeze, value: this.value }; }\n\
               __rspyts_export_requireHandle() { return { value: this.value }; }\n\
               __rspyts_export_close() { return { closed: true }; }\n\
               free() {}\n\
             }\n",
        )
        .unwrap();
        fs::write(package.join("native_bg.wasm"), []).unwrap();
        let packed = Command::new("npm")
            .args(["pack", "--json", "--pack-destination"])
            .arg(&root)
            .current_dir(&package)
            .env("NPM_CONFIG_CACHE", root.join("npm-cache"))
            .output()
            .unwrap();
        assert!(
            packed.status.success(),
            "npm pack failed:\n{}{}",
            String::from_utf8_lossy(&packed.stdout),
            String::from_utf8_lossy(&packed.stderr)
        );
        let metadata: Value = serde_json::from_slice(&packed.stdout).unwrap();
        let tarball = root.join(metadata[0]["filename"].as_str().unwrap());
        let consumer = root.join("consumer");
        fs::create_dir_all(&consumer).unwrap();
        fs::write(
            consumer.join("package.json"),
            "{\"private\":true,\"type\":\"module\"}\n",
        )
        .unwrap();
        let installed = Command::new("npm")
            .args([
                "install",
                "--ignore-scripts",
                "--no-package-lock",
                "--no-audit",
                "--no-fund",
            ])
            .arg(&tarball)
            .current_dir(&consumer)
            .env("NPM_CONFIG_CACHE", root.join("npm-cache"))
            .output()
            .unwrap();
        assert!(
            installed.status.success(),
            "npm install failed:\n{}{}",
            String::from_utf8_lossy(&installed.stdout),
            String::from_utf8_lossy(&installed.stderr)
        );
        fs::write(
            consumer.join("acceptance.js"),
            "import init, { deepFreeze, run, DomainError, Runner } from 'sample';\n\
             import { translateError } from 'sample/wire';\n\
             await init();\n\
             if (deepFreeze() !== 'public') throw new Error('public helper-like export failed');\n\
             try { run('n', 't', 'g', 'd'); throw new Error('expected typed error'); }\n\
             catch (error) { if (!(error instanceof DomainError) || error.code !== 'failed') throw error; }\n\
             const direct = new Runner('direct');\n\
             if (direct.handle('frozen').deepFreeze !== 'frozen') throw new Error('handle collision');\n\
             if (direct.requireHandle().value !== 'direct') throw new Error('guard collision');\n\
             if (!direct.close().closed) throw new Error('close collision');\n\
             const factory = Runner.fromValue('left', 'right');\n\
             if (factory.requireHandle().value !== 'left:right') throw new Error('factory collision');\n\
             if (translateError !== 'wire') throw new Error('static helper-like export failed');\n\
             direct.free(); factory.free();\n",
        )
        .unwrap();
        let executed = Command::new("node")
            .arg(consumer.join("acceptance.js"))
            .current_dir(&consumer)
            .output()
            .unwrap();
        assert!(
            executed.status.success(),
            "packed runtime failed:\n{}{}",
            String::from_utf8_lossy(&executed.stdout),
            String::from_utf8_lossy(&executed.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wasm_init_coalesces_concurrent_calls_and_retries_after_rejection() {
        if !Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return;
        }
        let contract = resolved(
            Manifest {
                ir_version: 4,
                crate_name: "sample".into(),
                crate_version: "1.0.0".into(),
                module_name: "native".into(),
                imports: vec![],
                types: vec![],
                errors: vec![],
                functions: vec![],
                resources: vec![],
                constants: vec![],
            },
            TypeScriptMode::Wasm,
        );
        let runtime = wasm_runtime(&contract, "sha256:test");
        assert!(runtime.contains("let __rspyts_initialization_promise;"));
        assert!(runtime.contains("export default function init(__rspyts_input)"));
        assert!(runtime.contains("__rspyts_initialize_native({ module_or_path: __rspyts_input })"));
        assert!(!runtime.contains("__rspyts_initialize_native(__rspyts_input)"));
        assert!(!runtime.contains("export default async function"));
        assert!(runtime.contains("__rspyts_initialization_promise = void 0;"));

        let root = std::env::temp_dir().join(format!(
            "rspyts-typescript-init-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("package.json"), "{\"type\":\"module\"}\n").unwrap();
        fs::write(root.join("index.js"), runtime).unwrap();
        fs::write(
            root.join("native.js"),
            "export let calls = 0;\n\
             export default function initialize(input) {\n\
               calls += 1;\n\
               if (input?.module_or_path === 'fail') return Promise.reject(new Error('boom'));\n\
               return Promise.resolve({ call: calls });\n\
             }\n",
        )
        .unwrap();
        fs::write(
            root.join("acceptance.js"),
            "import init from './index.js';\n\
             import { calls } from './native.js';\n\
             const first = init('fail');\n\
             const concurrent = init('ignored');\n\
             if (!(first instanceof Promise) || first !== concurrent) \
               throw new Error('concurrent calls did not share one Promise');\n\
             await first.then(\n\
               () => { throw new Error('expected initialization rejection'); },\n\
               () => undefined,\n\
             );\n\
             if (calls !== 1) throw new Error(`expected one first attempt, received ${calls}`);\n\
             const retry = init('ok');\n\
             if (retry === first) throw new Error('rejection did not clear cached Promise');\n\
             const result = await retry;\n\
             if (result.call !== 2 || calls !== 2) throw new Error('retry did not initialize once');\n\
             if (init('ignored-again') !== retry) throw new Error('success was not cached');\n\
             if (calls !== 2) throw new Error('cached success initialized again');\n",
        )
        .unwrap();
        let result = Command::new("node")
            .arg(root.join("acceptance.js"))
            .output()
            .expect("Node is required to verify generated TypeScript initialization");
        assert!(
            result.status.success(),
            "generated WASM init cache failed:\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
