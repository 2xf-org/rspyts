use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use rspyts::ir::{
    BufferElement, CargoPackageId, DefinitionId, FieldConstraints, FieldDef, Manifest, ScalarValue,
    Target, TypeDef, TypeRef, TypeShape,
};
use serde_json::Value;

use crate::config::TypeScriptMode;
use crate::emit::util::{python_tag_field, python_tagged_variant_name};
use crate::resolve::ResolvedContract;

const JSON_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub fn manifest(manifest: &Manifest) -> Result<()> {
    if manifest.ir_version != rspyts::ir::IR_VERSION {
        bail!(
            "contract IR version {} is unsupported; expected {}",
            manifest.ir_version,
            rspyts::ir::IR_VERSION
        );
    }
    if manifest.crate_name.is_empty() || manifest.crate_version.is_empty() {
        bail!("contract crate name and version cannot be empty");
    }
    if !identifier(&manifest.module_name) {
        bail!(
            "module name `{}` is not a host identifier",
            manifest.module_name
        );
    }

    let root_owner = CargoPackageId::new(manifest.crate_name.clone());
    for owner in manifest
        .types
        .iter()
        .map(|item| &item.owner)
        .chain(manifest.errors.iter().map(|item| &item.owner))
        .chain(manifest.functions.iter().map(|item| &item.owner))
        .chain(manifest.resources.iter().map(|item| &item.owner))
        .chain(manifest.constants.iter().map(|item| &item.owner))
    {
        if *owner != root_owner {
            bail!("local contract definition is owned by foreign package `{owner}`");
        }
    }
    unique_owned(
        "import owner",
        manifest.imports.iter().map(|import| import.owner.clone()),
    )?;
    for import in &manifest.imports {
        if import.owner == root_owner {
            bail!("contract cannot import its root Cargo package");
        }
        if import
            .types
            .iter()
            .any(|definition| definition.owner != import.owner)
            || import
                .errors
                .iter()
                .any(|definition| definition.owner != import.owner)
        {
            bail!(
                "import `{}` contains a mismatched definition owner",
                import.owner
            );
        }
    }

    unique(
        "type id",
        manifest.types.iter().map(|item| item.id.as_str()),
    )?;
    unique(
        "type name",
        manifest.types.iter().map(|item| item.name.as_str()),
    )?;
    unique(
        "error id",
        manifest.errors.iter().map(|item| item.id.as_str()),
    )?;
    unique(
        "error name",
        manifest.errors.iter().map(|item| item.name.as_str()),
    )?;
    unique(
        "resource id",
        manifest.resources.iter().map(|item| item.id.as_str()),
    )?;
    unique(
        "resource name",
        manifest.resources.iter().map(|item| item.name.as_str()),
    )?;

    let imported_types = manifest.imports.iter().flat_map(|import| &import.types);
    let imported_errors = manifest.imports.iter().flat_map(|import| &import.errors);
    let type_ids = manifest
        .types
        .iter()
        .map(type_identity)
        .chain(manifest.resources.iter().map(|item| DefinitionId {
            owner: item.owner.clone(),
            id: item.id.clone(),
        }))
        .chain(imported_types.clone().map(type_identity))
        .collect::<BTreeSet<_>>();
    let error_ids = manifest
        .errors
        .iter()
        .map(error_identity)
        .chain(imported_errors.clone().map(error_identity))
        .collect::<BTreeSet<_>>();

    let all_types = manifest
        .types
        .iter()
        .chain(imported_types)
        .collect::<Vec<_>>();
    let all_errors = manifest
        .errors
        .iter()
        .chain(imported_errors)
        .collect::<Vec<_>>();
    let type_definitions = all_types
        .iter()
        .map(|item| (type_identity(item), *item))
        .collect::<BTreeMap<_, _>>();
    unique_owned(
        "type identity",
        all_types.iter().map(|item| type_identity(item)),
    )?;
    unique(
        "type host name",
        all_types.iter().map(|item| item.name.as_str()),
    )?;
    unique_owned(
        "error identity",
        all_errors.iter().map(|item| error_identity(item)),
    )?;
    unique(
        "error host name",
        all_errors.iter().map(|item| item.name.as_str()),
    )?;
    for item in &all_types {
        validate_type(item, &type_ids, &type_definitions)?;
    }
    for error in &all_errors {
        if !identifier(&error.name) {
            bail!("error name `{}` is not a host identifier", error.name);
        }
        unique(
            "error code",
            error.variants.iter().map(|variant| variant.code.as_str()),
        )?;
        for variant in &error.variants {
            validate_fields(
                &variant.fields,
                &type_ids,
                &type_definitions,
                &format!("error variant {}", variant.code),
            )?;
        }
    }
    for function in &manifest.functions {
        validate_function(function, &type_ids, &error_ids)?;
        if matches!(function.target, Target::Static) {
            bail!(
                "function `{}` cannot target static output",
                function.host_name
            );
        }
    }
    for resource in &manifest.resources {
        if !identifier(&resource.name) {
            bail!("resource name `{}` is not a host identifier", resource.name);
        }
        if matches!(resource.target, Target::Static) {
            bail!("resource `{}` cannot target static output", resource.name);
        }
        if target_includes_python(resource.target)
            && !resource
                .constructors
                .iter()
                .any(|item| target_includes_python(item.target))
        {
            bail!(
                "resource `{}` requires at least one Python constructor",
                resource.name
            );
        }
        if target_includes_typescript(resource.target)
            && !resource
                .constructors
                .iter()
                .any(|item| target_includes_typescript(item.target))
        {
            bail!(
                "resource `{}` requires at least one TypeScript constructor",
                resource.name
            );
        }
        for constructor in &resource.constructors {
            if constructor.owner != root_owner {
                bail!("resource constructor is owned by a foreign Cargo package");
            }
            validate_child_target(
                resource.target,
                constructor.target,
                &resource.name,
                "constructor",
            )?;
            validate_function(constructor, &type_ids, &error_ids)?;
        }
        for method in &resource.methods {
            validate_child_target(resource.target, method.target, &resource.name, "method")?;
            validate_name(&method.host_name, "method")?;
            for param in &method.params {
                validate_name(&param.host_name, "method parameter")?;
                validate_ref(&param.ty, &type_ids)?;
            }
            validate_ref(&method.returns, &type_ids)?;
            validate_error_ref(method.error.as_ref(), &error_ids)?;
        }
        if target_includes_python(resource.target) {
            validate_python_native_resource_namespace(resource)?;
        }
    }
    reject_cycles(&all_types)?;
    validate_option_injectivity(manifest, &all_types, &all_errors, &type_definitions)?;
    for constant in &manifest.constants {
        validate_name(&constant.host_name, "constant")?;
        validate_ref(&constant.ty, &type_ids)?;
        validate_constant_value(
            &constant.value,
            &constant.ty,
            &type_definitions,
            &format!("constant `{}`", constant.host_name),
        )?;
    }

    Ok(())
}

fn validate_option_injectivity(
    manifest: &Manifest,
    types: &[&TypeDef],
    errors: &[&rspyts::ir::ErrorDef],
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
) -> Result<()> {
    for definition in types {
        match &definition.shape {
            TypeShape::Alias { target } => validate_option_reference(
                target,
                type_definitions,
                &format!("type `{}`", definition.name),
            )?,
            TypeShape::Struct { fields } => validate_option_fields(
                fields,
                type_definitions,
                &format!("type `{}`", definition.name),
            )?,
            TypeShape::StringEnum { variants } | TypeShape::TaggedEnum { variants, .. } => {
                for variant in variants {
                    validate_option_fields(
                        &variant.fields,
                        type_definitions,
                        &format!("type `{}::{}`", definition.name, variant.rust_name),
                    )?;
                }
            }
        }
    }
    for error in errors {
        for variant in &error.variants {
            validate_option_fields(
                &variant.fields,
                type_definitions,
                &format!("error `{}::{}`", error.name, variant.rust_name),
            )?;
        }
    }
    for function in &manifest.functions {
        validate_option_function(function, type_definitions, "function")?;
    }
    for resource in &manifest.resources {
        for constructor in &resource.constructors {
            validate_option_function(constructor, type_definitions, "resource constructor")?;
        }
        for method in &resource.methods {
            for parameter in &method.params {
                validate_option_reference(
                    &parameter.ty,
                    type_definitions,
                    &format!(
                        "resource method `{}::{}` parameter `{}`",
                        resource.name, method.rust_name, parameter.rust_name
                    ),
                )?;
            }
            validate_option_reference(
                &method.returns,
                type_definitions,
                &format!(
                    "resource method `{}::{}` return",
                    resource.name, method.rust_name
                ),
            )?;
        }
    }
    for constant in &manifest.constants {
        validate_option_reference(
            &constant.ty,
            type_definitions,
            &format!("constant `{}`", constant.host_name),
        )?;
    }
    Ok(())
}

fn validate_option_function(
    function: &rspyts::ir::FunctionDef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    kind: &str,
) -> Result<()> {
    for parameter in &function.params {
        validate_option_reference(
            &parameter.ty,
            type_definitions,
            &format!(
                "{kind} `{}` parameter `{}`",
                function.rust_name, parameter.rust_name
            ),
        )?;
    }
    validate_option_reference(
        &function.returns,
        type_definitions,
        &format!("{kind} `{}` return", function.rust_name),
    )
}

fn validate_option_fields(
    fields: &[FieldDef],
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    owner: &str,
) -> Result<()> {
    for field in fields {
        validate_option_reference(
            &field.ty,
            type_definitions,
            &format!("{owner} field `{}`", field.wire_name),
        )?;
    }
    Ok(())
}

fn validate_option_reference(
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    path: &str,
) -> Result<()> {
    match reference {
        TypeRef::Option { item } => {
            if reference_accepts_null(item, type_definitions, &mut BTreeSet::new()) {
                bail!("{path} uses a non-injective Option whose inner type also accepts null");
            }
            validate_option_reference(item, type_definitions, path)
        }
        TypeRef::List { item } => validate_option_reference(item, type_definitions, path),
        TypeRef::Map { value } => validate_option_reference(value, type_definitions, path),
        TypeRef::Tuple { items } => {
            for item in items {
                validate_option_reference(item, type_definitions, path)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn reference_accepts_null(
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    visited: &mut BTreeSet<DefinitionId>,
) -> bool {
    match reference {
        TypeRef::Unit | TypeRef::Json | TypeRef::Option { .. } => true,
        TypeRef::Named { identity } if visited.insert(identity.clone()) => {
            match type_definitions
                .get(identity)
                .map(|definition| &definition.shape)
            {
                Some(TypeShape::Alias { target }) => {
                    reference_accepts_null(target, type_definitions, visited)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn validate_constant_value(
    value: &Value,
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    path: &str,
) -> Result<()> {
    match reference {
        TypeRef::Unit => expect_constant_kind(value, path, "null", Value::is_null),
        TypeRef::Bool => expect_constant_kind(value, path, "boolean", Value::is_boolean),
        TypeRef::Int { signed, bits } => validate_constant_integer(value, *signed, *bits, path),
        TypeRef::Float { bits } => validate_constant_float(value, *bits, path),
        TypeRef::String => expect_constant_kind(value, path, "string", Value::is_string),
        TypeRef::DateTime => {
            let value = value.as_str().ok_or_else(|| {
                anyhow::anyhow!(
                    "{path} expected an aware RFC3339 datetime, found {}",
                    constant_kind(value)
                )
            })?;
            validate_datetime(value, path)
        }
        TypeRef::Json => validate_json_number_tree(value, path),
        TypeRef::Option { item } => {
            if value.is_null() {
                Ok(())
            } else {
                validate_constant_value(value, item, type_definitions, path)
            }
        }
        TypeRef::List { item } => {
            for (index, value) in constant_array(value, path)?.iter().enumerate() {
                validate_constant_value(
                    value,
                    item,
                    type_definitions,
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        }
        TypeRef::Map { value: item } => {
            for (key, value) in constant_object(value, path)? {
                validate_constant_value(
                    value,
                    item,
                    type_definitions,
                    &format!("{path}[{key:?}]"),
                )?;
            }
            Ok(())
        }
        TypeRef::Tuple { items } => {
            let values = constant_array(value, path)?;
            if values.len() != items.len() {
                bail!(
                    "{path} expected tuple with {} elements, found {}",
                    items.len(),
                    values.len()
                );
            }
            for (index, (value, item)) in values.iter().zip(items).enumerate() {
                validate_constant_value(
                    value,
                    item,
                    type_definitions,
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        }
        TypeRef::Named { identity } => {
            match type_definitions.get(identity).map(|item| &item.shape) {
                Some(TypeShape::Alias { target }) => {
                    validate_constant_value(value, target, type_definitions, path)
                }
                Some(TypeShape::Struct { fields }) => {
                    validate_constant_fields(value, fields, None, type_definitions, path)
                }
                Some(TypeShape::StringEnum { variants }) => {
                    let wire = value.as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "{path} expected a declared string enum variant, found {}",
                            constant_kind(value)
                        )
                    })?;
                    if variants.iter().any(|variant| variant.wire_name == wire) {
                        Ok(())
                    } else {
                        bail!("{path} expected a declared string enum variant, found {wire:?}")
                    }
                }
                Some(TypeShape::TaggedEnum { tag, variants }) => {
                    let values = constant_object(value, path)?;
                    let tag_path = format!("{path}[{tag:?}]");
                    let wire = values
                        .get(tag)
                        .ok_or_else(|| {
                            anyhow::anyhow!("{tag_path} expected required tag, found missing")
                        })?
                        .as_str()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "{tag_path} expected variant string, found {}",
                                constant_kind(&values[tag])
                            )
                        })?;
                    let variant = variants
                        .iter()
                        .find(|variant| variant.wire_name == wire)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "{tag_path} expected a declared tagged variant, found {wire:?}"
                            )
                        })?;
                    validate_constant_fields(
                        value,
                        &variant.fields,
                        Some(tag),
                        type_definitions,
                        path,
                    )
                }
                None => {
                    bail!("{path} references type `{identity}` without a constant value schema")
                }
            }
        }
        TypeRef::Bytes | TypeRef::FixedBytes { .. } => {
            let values = constant_array(value, path)?;
            if let TypeRef::FixedBytes { length } = reference
                && values.len() as u64 != *length
            {
                bail!(
                    "{path} expected exactly {length} bytes, found {}",
                    values.len()
                );
            }
            for (index, value) in values.iter().enumerate() {
                validate_constant_integer(value, false, 8, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        TypeRef::Buffer { element } => {
            let values = constant_array(value, path)?;
            for (index, value) in values.iter().enumerate() {
                let item_path = format!("{path}[{index}]");
                match element {
                    BufferElement::U8 => validate_constant_integer(value, false, 8, &item_path)?,
                    BufferElement::I8 => validate_constant_integer(value, true, 8, &item_path)?,
                    BufferElement::U16 => validate_constant_integer(value, false, 16, &item_path)?,
                    BufferElement::I16 => validate_constant_integer(value, true, 16, &item_path)?,
                    BufferElement::U32 => validate_constant_integer(value, false, 32, &item_path)?,
                    BufferElement::I32 => validate_constant_integer(value, true, 32, &item_path)?,
                    BufferElement::U64 => validate_constant_integer(value, false, 64, &item_path)?,
                    BufferElement::I64 => validate_constant_integer(value, true, 64, &item_path)?,
                    BufferElement::F32 => validate_constant_float(value, 32, &item_path)?,
                    BufferElement::F64 => validate_constant_float(value, 64, &item_path)?,
                }
            }
            Ok(())
        }
    }
}

fn validate_constant_fields(
    value: &Value,
    fields: &[FieldDef],
    tag: Option<&str>,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    path: &str,
) -> Result<()> {
    let values = constant_object(value, path)?;
    let allowed = fields
        .iter()
        .map(|field| field.wire_name.as_str())
        .chain(tag)
        .collect::<BTreeSet<_>>();
    if let Some(key) = values.keys().find(|key| !allowed.contains(key.as_str())) {
        bail!("{path}[{key:?}] expected a declared field, found unknown field");
    }
    for field in fields {
        let field_path = format!("{path}.{}", field.wire_name);
        match values.get(&field.wire_name) {
            Some(value) => {
                validate_constant_value(value, &field.ty, type_definitions, &field_path)?;
                validate_constant_constraints(
                    value,
                    &field.ty,
                    &field.constraints,
                    type_definitions,
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

fn validate_constant_constraints(
    value: &Value,
    ty: &TypeRef,
    constraints: &FieldConstraints,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    path: &str,
) -> Result<()> {
    if value.is_null() && constant_type_allows_null(ty, type_definitions, &mut BTreeSet::new()) {
        return Ok(());
    }
    if let Some(literal) = &constraints.literal {
        let literal = scalar_json(literal);
        validate_constant_value(&literal, ty, type_definitions, path)?;
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
                constant_kind(value)
            ),
        };
        if constraints
            .min_length
            .is_some_and(|minimum| length < minimum)
            || constraints
                .max_length
                .is_some_and(|maximum| length > maximum)
        {
            bail!("{path} has length {length} outside its declared constraints");
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

fn constant_type_allows_null(
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    visited: &mut BTreeSet<DefinitionId>,
) -> bool {
    match reference {
        TypeRef::Option { .. } => true,
        TypeRef::Named { identity } if visited.insert(identity.clone()) => {
            match type_definitions
                .get(identity)
                .map(|definition| &definition.shape)
            {
                Some(TypeShape::Alias { target }) => {
                    constant_type_allows_null(target, type_definitions, visited)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn validate_constant_integer(value: &Value, signed: bool, bits: u16, path: &str) -> Result<()> {
    if !matches!(bits, 8 | 16 | 32 | 64) {
        bail!("{path} uses unsupported {bits}-bit integer type");
    }
    if signed {
        let Some(value) = value.as_i64() else {
            bail!(
                "{path} expected a signed {bits}-bit integer, found {}",
                constant_kind(value)
            );
        };
        let valid = match bits {
            8 => i8::try_from(value).is_ok(),
            16 => i16::try_from(value).is_ok(),
            32 => i32::try_from(value).is_ok(),
            64 => true,
            _ => unreachable!(),
        };
        if !valid {
            bail!("{path} expected a signed {bits}-bit integer, found {value}");
        }
    } else {
        let Some(value) = value.as_u64() else {
            bail!(
                "{path} expected an unsigned {bits}-bit integer, found {}",
                constant_kind(value)
            );
        };
        let valid = match bits {
            8 => u8::try_from(value).is_ok(),
            16 => u16::try_from(value).is_ok(),
            32 => u32::try_from(value).is_ok(),
            64 => true,
            _ => unreachable!(),
        };
        if !valid {
            bail!("{path} expected an unsigned {bits}-bit integer, found {value}");
        }
    }
    Ok(())
}

fn validate_constant_float(value: &Value, bits: u16, path: &str) -> Result<()> {
    if !matches!(bits, 32 | 64) {
        bail!("{path} uses unsupported {bits}-bit float type");
    }
    let Some(value) = value.as_f64() else {
        bail!(
            "{path} expected a finite f{bits}, found {}",
            constant_kind(value)
        );
    };
    if !value.is_finite() || bits == 32 && !(value as f32).is_finite() {
        bail!("{path} expected a finite f{bits}, found {value}");
    }
    Ok(())
}

fn scalar_json(value: &ScalarValue) -> Value {
    match value {
        ScalarValue::Bool(value) => Value::Bool(*value),
        ScalarValue::I64(value) => Value::Number((*value).into()),
        ScalarValue::String(value) => Value::String(value.clone()),
    }
}

fn constant_array<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{path} expected array, found {}", constant_kind(value)))
}

fn constant_object<'a>(value: &'a Value, path: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{path} expected object, found {}", constant_kind(value)))
}

fn expect_constant_kind(
    value: &Value,
    path: &str,
    expected: &str,
    predicate: impl FnOnce(&Value) -> bool,
) -> Result<()> {
    if predicate(value) {
        Ok(())
    } else {
        bail!("{path} expected {expected}, found {}", constant_kind(value))
    }
}

fn constant_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn validate_json_number_tree(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_json_number_tree(value, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_json_number_tree(value, &format!("{path}[{key:?}]"))?;
            }
        }
        Value::Number(number) => {
            let unsafe_integer = number.as_i64().is_some_and(|value| {
                value < -(JSON_MAX_SAFE_INTEGER as i64) || value > JSON_MAX_SAFE_INTEGER as i64
            }) || number
                .as_u64()
                .is_some_and(|value| value > JSON_MAX_SAFE_INTEGER)
                || number.as_f64().is_some_and(|value| {
                    value.fract() == 0.0
                        && (value < -(JSON_MAX_SAFE_INTEGER as f64)
                            || value > JSON_MAX_SAFE_INTEGER as f64)
                });
            if unsafe_integer {
                bail!(
                    "{path} contains JSON integer {number} outside JavaScript's safe integer range"
                );
            }
        }
        _ => {}
    }
    Ok(())
}

const fn target_includes_python(target: Target) -> bool {
    matches!(target, Target::Both | Target::Python)
}

fn validate_python_native_resource_namespace(resource: &rspyts::ir::ResourceDef) -> Result<()> {
    let constructors = resource
        .constructors
        .iter()
        .filter(|constructor| target_includes_python(constructor.target))
        .collect::<Vec<_>>();
    let primary = constructors
        .iter()
        .position(|constructor| constructor.rust_name == "new")
        .or((!constructors.is_empty()).then_some(0));
    let mut members = BTreeMap::from([(
        "close".to_owned(),
        "generated Python native resource member `close`".to_owned(),
    )]);
    for (index, constructor) in constructors.into_iter().enumerate() {
        if primary != Some(index) {
            register_resource_host_member(
                &mut members,
                &constructor.host_name,
                &format!("constructor `{}`", constructor.rust_name),
                &resource.name,
                "Python native",
            )?;
        }
    }
    for method in resource
        .methods
        .iter()
        .filter(|method| target_includes_python(method.target))
    {
        register_resource_host_member(
            &mut members,
            &method.host_name,
            &format!("method `{}`", method.rust_name),
            &resource.name,
            "Python native",
        )?;
    }
    Ok(())
}

fn register_resource_host_member(
    members: &mut BTreeMap<String, String>,
    name: &str,
    origin: &str,
    resource: &str,
    host: &str,
) -> Result<()> {
    if let Some(previous) = members.insert(name.to_owned(), origin.to_owned()) {
        bail!(
            "{host} resource `{resource}` member `{name}` from {origin} collides with {previous}"
        );
    }
    Ok(())
}

const fn target_includes_typescript(target: Target) -> bool {
    matches!(target, Target::Both | Target::Typescript)
}

fn validate_child_target(
    resource_target: Target,
    child_target: Target,
    resource: &str,
    kind: &str,
) -> Result<()> {
    if matches!(child_target, Target::Static)
        || (target_includes_python(child_target) && !target_includes_python(resource_target))
        || (target_includes_typescript(child_target)
            && !target_includes_typescript(resource_target))
    {
        bail!("resource `{resource}` has a {kind} outside its target scope");
    }
    Ok(())
}

fn validate_type(
    item: &TypeDef,
    type_ids: &BTreeSet<DefinitionId>,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
) -> Result<()> {
    validate_name(&item.name, "type")?;
    match &item.shape {
        TypeShape::Struct { fields } => {
            validate_fields(fields, type_ids, type_definitions, &item.name)
        }
        TypeShape::StringEnum { variants } => {
            unique(
                "enum wire value",
                variants.iter().map(|variant| variant.wire_name.as_str()),
            )?;
            if variants.iter().any(|variant| !variant.fields.is_empty()) {
                bail!("string enum `{}` cannot have variant fields", item.name);
            }
            Ok(())
        }
        TypeShape::TaggedEnum { tag, variants } => {
            if tag.is_empty() || variants.is_empty() {
                bail!("tagged enum `{}` requires a tag and variants", item.name);
            }
            unique(
                "enum wire value",
                variants.iter().map(|variant| variant.wire_name.as_str()),
            )?;
            for variant in variants {
                if variant.fields.iter().any(|field| field.wire_name == *tag) {
                    bail!(
                        "tagged enum `{}::{}` field wire name `{tag}` duplicates its discriminator tag",
                        item.name,
                        variant.rust_name
                    );
                }
                validate_fields(
                    &variant.fields,
                    type_ids,
                    type_definitions,
                    &format!("{}::{}", item.name, variant.rust_name),
                )?;
            }
            Ok(())
        }
        TypeShape::Alias { target } => validate_ref(target, type_ids),
    }
}

pub(crate) fn python_manifest(manifest: &Manifest) -> Result<()> {
    validate_python_identifier(&manifest.module_name, "native module")?;
    if matches!(
        manifest.module_name.as_str(),
        "__init__" | "models" | "codecs" | "errors" | "functions" | "resources" | "constants"
    ) {
        bail!(
            "Python native module name `{}` collides with a generated module",
            manifest.module_name
        );
    }

    let mut native_symbols = BTreeMap::new();
    register_python_name(
        &mut native_symbols,
        "BufferPayload",
        "generated native buffer class",
    )?;
    for function in manifest
        .functions
        .iter()
        .filter(|item| target_includes_python(item.target))
    {
        register_python_name(
            &mut native_symbols,
            &function.host_name,
            &format!("function `{}` native host", function.rust_name),
        )?;
    }
    for resource in manifest
        .resources
        .iter()
        .filter(|item| target_includes_python(item.target))
    {
        register_python_name(
            &mut native_symbols,
            &resource.name,
            &format!("resource `{}` native class", resource.name),
        )?;
    }

    let imported_types = manifest.imports.iter().flat_map(|import| &import.types);
    let imported_errors = manifest.imports.iter().flat_map(|import| &import.errors);
    let all_types = manifest
        .types
        .iter()
        .chain(imported_types)
        .collect::<Vec<_>>();
    let all_errors = manifest
        .errors
        .iter()
        .chain(imported_errors)
        .collect::<Vec<_>>();
    let mut exports = BTreeMap::<String, String>::new();
    for name in [
        "models",
        "codecs",
        "errors",
        "functions",
        "resources",
        "constants",
        manifest.module_name.as_str(),
    ] {
        register_python_name(
            &mut exports,
            name,
            &format!("generated Python module `{name}`"),
        )?;
    }
    for name in [
        "JsonValue",
        "ContractError",
        "ResourceClosedError",
        "CONTRACT_FINGERPRINT",
    ] {
        register_python_export(
            &mut exports,
            name,
            format!("generated support symbol `{name}`"),
        )?;
    }
    for item in &all_types {
        let origin = format!("type `{}`", item.name);
        register_python_export(&mut exports, &item.name, origin.clone())?;
        match &item.shape {
            TypeShape::Struct { fields } => {
                validate_python_fields(fields, &origin)?;
            }
            TypeShape::StringEnum { variants } => {
                let mut members = BTreeMap::new();
                for variant in variants {
                    let variant_origin = format!("{origin} variant `{}`", variant.rust_name);
                    validate_python_enum_member(&variant.rust_name, &variant_origin)?;
                    register_python_name(&mut members, &variant.rust_name, &variant_origin)?;
                }
            }
            TypeShape::TaggedEnum { variants, .. } => {
                for variant in variants {
                    let variant_origin = format!("{origin} variant `{}`", variant.rust_name);
                    validate_python_fields(&variant.fields, &variant_origin)?;
                    if item.owner == CargoPackageId::new(manifest.crate_name.clone()) {
                        let class_name = python_tagged_variant_name(&item.name, &variant.rust_name);
                        register_python_export(
                            &mut exports,
                            &class_name,
                            format!("{variant_origin} generated class"),
                        )?;
                    }
                }
                let tag_field = python_tag_field(
                    match &item.shape {
                        TypeShape::TaggedEnum { tag, .. } => tag,
                        _ => unreachable!(),
                    },
                    variants,
                );
                validate_python_model_field_identifier(
                    &tag_field,
                    &format!("{origin} generated discriminator field"),
                )?;
            }
            TypeShape::Alias { .. } => {}
        }
    }

    for error in all_errors {
        let origin = format!("error `{}`", error.name);
        register_python_export(&mut exports, &error.name, origin.clone())?;
        for variant in &error.variants {
            validate_python_fields(
                &variant.fields,
                &format!("{origin} variant `{}`", variant.rust_name),
            )?;
        }
    }

    for function in manifest
        .functions
        .iter()
        .filter(|item| target_includes_python(item.target))
    {
        let origin = format!("function `{}`", function.rust_name);
        validate_python_callable(function, &origin, &[])?;
        register_python_export(&mut exports, &function.rust_name, origin.clone())?;
    }

    for resource in manifest
        .resources
        .iter()
        .filter(|item| target_includes_python(item.target))
    {
        let origin = format!("resource `{}`", resource.name);
        register_python_export(&mut exports, &resource.name, origin.clone())?;
        let mut members = BTreeMap::new();
        let constructors = resource
            .constructors
            .iter()
            .filter(|item| target_includes_python(item.target))
            .collect::<Vec<_>>();
        let primary = constructors
            .iter()
            .position(|constructor| constructor.rust_name == "new")
            .or((!constructors.is_empty()).then_some(0));
        for (index, constructor) in constructors.into_iter().enumerate() {
            let member_origin = format!("{origin} constructor `{}`", constructor.rust_name);
            if primary == Some(index) {
                validate_python_params(&constructor.params, &member_origin, &["self"])?;
                continue;
            }
            validate_python_resource_member_name(&constructor.rust_name, &member_origin)?;
            validate_python_callable(constructor, &member_origin, &["cls"])?;
            register_python_name(&mut members, &constructor.rust_name, &member_origin)?;
        }
        for method in resource
            .methods
            .iter()
            .filter(|item| target_includes_python(item.target))
        {
            let member_origin = format!("{origin} method `{}`", method.rust_name);
            validate_python_resource_member_name(&method.rust_name, &member_origin)?;
            validate_python_identifier(
                &method.host_name,
                &format!("{member_origin} native host name"),
            )?;
            validate_python_params(&method.params, &member_origin, &["self"])?;
            register_python_name(&mut members, &method.rust_name, &member_origin)?;
        }
    }

    for constant in manifest
        .constants
        .iter()
        .filter(|item| target_includes_python(item.target))
    {
        let origin = format!("constant `{}`", constant.host_name);
        register_python_export(&mut exports, &constant.host_name, origin.clone())?;
    }
    Ok(())
}

pub(crate) fn typescript_contract(contract: &ResolvedContract, mode: TypeScriptMode) -> Result<()> {
    match mode {
        TypeScriptMode::Static => validate_typescript_static_surface(
            contract,
            "static package",
            &crate::emit::static_typescript_type_ids(contract),
            None,
        ),
        TypeScriptMode::Wasm => {
            validate_typescript_wasm_surface(contract)?;
            let (wire_types, wire_constants) = crate::emit::wire_typescript_surface(contract);
            validate_typescript_static_surface(
                contract,
                "WASM `./wire` package",
                &wire_types,
                Some(&wire_constants),
            )
        }
    }
}

fn validate_typescript_wasm_surface(contract: &ResolvedContract) -> Result<()> {
    let surface = "WASM package";
    let manifest = &contract.manifest;
    let mut types = BTreeMap::new();
    let mut values = BTreeMap::new();

    for (name, origin) in [
        ("JsonValue", "generated type `JsonValue`"),
        ("InitInput", "generated type `InitInput`"),
        ("InitOutput", "generated type `InitOutput`"),
        (
            "ResourceClosedError",
            "generated class `ResourceClosedError`",
        ),
        ("globalThis", "ambient namespace `globalThis`"),
        (
            "RequestInfo",
            "ambient type `RequestInfo` used by `InitInput`",
        ),
        ("URL", "ambient type `URL` used by `InitInput`"),
        ("Response", "ambient type `Response` used by `InitInput`"),
        (
            "BufferSource",
            "ambient type `BufferSource` used by `InitInput`",
        ),
        (
            "WebAssembly",
            "ambient namespace `WebAssembly` used by initialization declarations",
        ),
        (
            "Promise",
            "ambient type `Promise` used by initialization declarations",
        ),
    ] {
        register_typescript_name(&mut types, name, origin, surface)?;
    }
    for (name, origin) in [
        (
            "CONTRACT_FINGERPRINT",
            "generated constant `CONTRACT_FINGERPRINT`",
        ),
        ("init", "generated default initializer `init`"),
        (
            "ResourceClosedError",
            "generated class `ResourceClosedError`",
        ),
        ("globalThis", "ambient runtime binding `globalThis`"),
    ] {
        register_typescript_name(&mut values, name, origin, surface)?;
    }

    for item in &manifest.types {
        let origin = format!("type `{}`", item.name);
        register_typescript_name(&mut types, &item.name, &origin, surface)?;
        validate_typescript_enum_members(item, surface)?;
        if matches!(item.shape, TypeShape::StringEnum { .. }) {
            register_typescript_name(&mut values, &item.name, &origin, surface)?;
        }
    }
    for item in contract.foreign_types.values() {
        register_typescript_name(
            &mut types,
            &item.name,
            &format!("imported type `{}`", item.name),
            surface,
        )?;
        if matches!(item.shape, TypeShape::StringEnum { .. }) {
            register_typescript_name(
                &mut values,
                &item.name,
                &format!("imported string enum `{}`", item.name),
                surface,
            )?;
        }
    }
    for error in &manifest.errors {
        let origin = format!("error `{}`", error.name);
        register_typescript_name(&mut types, &error.name, &origin, surface)?;
        register_typescript_name(&mut values, &error.name, &origin, surface)?;
    }
    for error in contract.foreign_errors.values() {
        let origin = format!("imported error `{}`", error.name);
        register_typescript_name(&mut types, &error.name, &origin, surface)?;
        register_typescript_name(&mut values, &error.name, &origin, surface)?;
    }

    for function in manifest
        .functions
        .iter()
        .filter(|function| target_includes_typescript(function.target))
    {
        let origin = format!("function `{}`", function.rust_name);
        register_typescript_name(&mut values, &function.host_name, &origin, surface)?;
        validate_typescript_params(&function.params, &origin, surface)?;
    }

    for resource in manifest
        .resources
        .iter()
        .filter(|resource| target_includes_typescript(resource.target))
    {
        let origin = format!("resource `{}`", resource.name);
        register_typescript_name(&mut types, &resource.name, &origin, surface)?;
        register_typescript_name(&mut values, &resource.name, &origin, surface)?;
        validate_typescript_resource(resource, surface)?;
    }

    for constant in manifest
        .constants
        .iter()
        .filter(|constant| target_includes_typescript(constant.target))
    {
        let origin = format!("constant `{}`", constant.rust_name);
        register_typescript_name(&mut values, &constant.host_name, &origin, surface)?;
    }
    Ok(())
}

fn validate_typescript_static_surface(
    contract: &ResolvedContract,
    surface: &str,
    emitted_types: &BTreeSet<DefinitionId>,
    emitted_constants: Option<&BTreeSet<usize>>,
) -> Result<()> {
    let mut types = BTreeMap::new();
    let mut values = BTreeMap::new();
    register_typescript_name(
        &mut types,
        "JsonValue",
        "generated type `JsonValue`",
        surface,
    )?;
    for (name, origin) in [
        (
            "CONTRACT_FINGERPRINT",
            "generated constant `CONTRACT_FINGERPRINT`",
        ),
        ("globalThis", "ambient runtime binding `globalThis`"),
    ] {
        register_typescript_name(&mut values, name, origin, surface)?;
    }

    for item in contract
        .manifest
        .types
        .iter()
        .chain(contract.foreign_types.values())
        .filter(|item| emitted_types.contains(&type_identity(item)))
    {
        let local = item.owner == CargoPackageId::new(contract.manifest.crate_name.clone());
        let origin = if local {
            format!("type `{}`", item.name)
        } else {
            format!("imported type `{}`", item.name)
        };
        register_typescript_name(&mut types, &item.name, &origin, surface)?;
        if local {
            validate_typescript_enum_members(item, surface)?;
        }
        if matches!(item.shape, TypeShape::StringEnum { .. }) {
            register_typescript_name(&mut values, &item.name, &origin, surface)?;
        }
    }
    for constant in contract
        .manifest
        .constants
        .iter()
        .enumerate()
        .filter(|(index, constant)| {
            matches!(
                constant.target,
                Target::Both | Target::Typescript | Target::Static
            ) && emitted_constants.is_none_or(|indices| indices.contains(index))
        })
        .map(|(_, constant)| constant)
    {
        let origin = format!("constant `{}`", constant.rust_name);
        register_typescript_name(&mut values, &constant.host_name, &origin, surface)?;
    }
    Ok(())
}

fn validate_typescript_enum_members(item: &TypeDef, surface: &str) -> Result<()> {
    let TypeShape::StringEnum { variants } = &item.shape else {
        return Ok(());
    };
    let mut members = BTreeMap::new();
    for variant in variants {
        register_typescript_member_name(
            &mut members,
            &variant.rust_name,
            &format!(
                "string enum `{}` variant `{}`",
                item.name, variant.rust_name
            ),
            surface,
        )?;
    }
    Ok(())
}

fn validate_typescript_resource(resource: &rspyts::ir::ResourceDef, surface: &str) -> Result<()> {
    let origin = format!("resource `{}`", resource.name);
    let constructors = resource
        .constructors
        .iter()
        .filter(|constructor| target_includes_typescript(constructor.target))
        .collect::<Vec<_>>();
    let primary = constructors
        .iter()
        .position(|constructor| constructor.rust_name == "new")
        .or((!constructors.is_empty()).then_some(0));
    let mut static_members = BTreeMap::new();
    for (name, generated) in [
        ("constructor", "generated constructor"),
        ("prototype", "generated class prototype"),
    ] {
        register_typescript_member_name(&mut static_members, name, generated, &origin)?;
    }
    let mut instance_members = BTreeMap::new();
    for (name, generated) in [
        ("constructor", "generated constructor"),
        ("free", "generated lifecycle method"),
    ] {
        register_typescript_member_name(&mut instance_members, name, generated, &origin)?;
    }

    for (index, constructor) in constructors.into_iter().enumerate() {
        let member_origin = format!("{origin} constructor `{}`", constructor.rust_name);
        if primary == Some(index) {
            validate_typescript_params(&constructor.params, &member_origin, surface)?;
            continue;
        }
        register_typescript_member_name(
            &mut static_members,
            &constructor.host_name,
            &member_origin,
            &origin,
        )?;
        validate_typescript_params(&constructor.params, &member_origin, surface)?;
    }

    for method in resource
        .methods
        .iter()
        .filter(|method| target_includes_typescript(method.target))
    {
        let member_origin = format!("{origin} method `{}`", method.rust_name);
        register_typescript_member_name(
            &mut instance_members,
            &method.host_name,
            &member_origin,
            &origin,
        )?;
        validate_typescript_params(&method.params, &member_origin, surface)?;
    }
    Ok(())
}

fn validate_typescript_params(
    params: &[rspyts::ir::ParamDef],
    owner: &str,
    surface: &str,
) -> Result<()> {
    let mut names = BTreeMap::new();
    for param in params {
        let origin = format!("{owner} parameter `{}`", param.host_name);
        validate_typescript_identifier(&param.host_name, &origin, surface)?;
        register_typescript_name(&mut names, &param.host_name, &origin, surface)?;
    }
    Ok(())
}

fn register_typescript_name(
    names: &mut BTreeMap<String, String>,
    value: &str,
    origin: &str,
    surface: &str,
) -> Result<()> {
    validate_typescript_identifier(value, origin, surface)?;
    if let Some(previous) = names.insert(value.to_owned(), origin.to_owned()) {
        bail!("TypeScript {surface} name `{value}` from {origin} collides with {previous}");
    }
    Ok(())
}

fn register_typescript_member_name(
    names: &mut BTreeMap<String, String>,
    value: &str,
    origin: &str,
    surface: &str,
) -> Result<()> {
    if !identifier(value) {
        bail!("TypeScript {surface} {origin} name `{value}` is not an identifier");
    }
    if value.starts_with("__rspyts_") {
        bail!("TypeScript {surface} {origin} name `{value}` uses the reserved rspyts prefix");
    }
    if let Some(previous) = names.insert(value.to_owned(), origin.to_owned()) {
        bail!("TypeScript {surface} name `{value}` from {origin} collides with {previous}");
    }
    Ok(())
}

fn validate_typescript_identifier(value: &str, origin: &str, surface: &str) -> Result<()> {
    if !identifier(value) {
        bail!("TypeScript {surface} {origin} name `{value}` is not an identifier");
    }
    if value.starts_with("__rspyts_") {
        bail!("TypeScript {surface} {origin} name `{value}` uses the reserved rspyts prefix");
    }
    if typescript_reserved_word(value) {
        bail!("TypeScript {surface} {origin} name `{value}` is reserved");
    }
    Ok(())
}

fn typescript_reserved_word(value: &str) -> bool {
    matches!(
        value,
        "arguments"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "eval"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn validate_python_resource_member_name(value: &str, origin: &str) -> Result<()> {
    if matches!(
        value,
        "handle" | "close" | "__init__" | "__enter__" | "__exit__" | "__new__"
    ) {
        bail!("Python {origin} name `{value}` is reserved by the resource wrapper");
    }
    validate_python_identifier(value, origin)
}

fn validate_python_enum_member(value: &str, origin: &str) -> Result<()> {
    validate_python_identifier(value, origin)?;
    if value == "mro" || value.starts_with('_') && value.ends_with('_') {
        bail!("Python {origin} name `{value}` is reserved by Enum");
    }
    Ok(())
}

fn validate_python_fields(fields: &[FieldDef], owner: &str) -> Result<()> {
    let mut names = BTreeMap::new();
    for field in fields {
        let origin = format!("{owner} field `{}`", field.wire_name);
        validate_python_model_field_identifier(&field.rust_name, &origin)?;
        register_python_name(&mut names, &field.rust_name, &origin)?;
    }
    Ok(())
}

fn validate_python_callable(
    function: &rspyts::ir::FunctionDef,
    origin: &str,
    reserved_params: &[&str],
) -> Result<()> {
    validate_python_identifier(&function.rust_name, origin)?;
    validate_python_identifier(&function.host_name, &format!("{origin} native host name"))?;
    validate_python_params(&function.params, origin, reserved_params)
}

fn validate_python_params(
    params: &[rspyts::ir::ParamDef],
    owner: &str,
    reserved: &[&str],
) -> Result<()> {
    let mut names = BTreeMap::new();
    for param in params {
        let origin = format!("{owner} parameter `{}`", param.rust_name);
        validate_python_identifier(&param.rust_name, &origin)?;
        if reserved.contains(&param.rust_name.as_str()) {
            bail!(
                "Python {origin} collides with generated resource local `{}`",
                param.rust_name
            );
        }
        register_python_name(&mut names, &param.rust_name, &origin)?;
    }
    Ok(())
}

fn validate_python_model_field_identifier(value: &str, origin: &str) -> Result<()> {
    if !identifier(value) || python_reserved_word(value) || value.starts_with('_') {
        bail!("Python {origin} name `{value}` is not a public identifier");
    }
    if value == "model_config"
        || value == "model_fields"
        || value == "model_extra"
        || value == "model_computed_fields"
        || value.starts_with("model_dump")
        || value.starts_with("model_validate")
    {
        bail!("Python {origin} name `{value}` is reserved by Pydantic");
    }
    Ok(())
}

fn validate_python_identifier(value: &str, origin: &str) -> Result<()> {
    if !identifier(value) || python_reserved_word(value) {
        bail!("Python {origin} name `{value}` is not an identifier");
    }
    if value.starts_with("__rspyts_") {
        bail!("Python {origin} name `{value}` uses the reserved rspyts prefix");
    }
    Ok(())
}

fn python_reserved_word(value: &str) -> bool {
    matches!(
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
            | "case"
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
            | "match"
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
    )
}

fn register_python_export(
    names: &mut BTreeMap<String, String>,
    value: &str,
    origin: String,
) -> Result<()> {
    validate_python_identifier(value, &origin)?;
    register_python_name(names, value, &origin)
}

fn register_python_name(
    names: &mut BTreeMap<String, String>,
    value: &str,
    origin: &str,
) -> Result<()> {
    if let Some(previous) = names.insert(value.to_owned(), origin.to_owned()) {
        bail!("Python name `{value}` from {origin} collides with {previous}");
    }
    Ok(())
}

fn validate_fields(
    fields: &[FieldDef],
    type_ids: &BTreeSet<DefinitionId>,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    owner: &str,
) -> Result<()> {
    unique(
        "field wire name",
        fields.iter().map(|field| field.wire_name.as_str()),
    )?;
    for field in fields {
        if field.rust_name.is_empty() || field.wire_name.is_empty() {
            bail!("{owner} has an empty field name");
        }
        validate_ref(&field.ty, type_ids)?;
        validate_field_contract(field, type_definitions, owner)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    Bool,
    Integer { signed: bool, bits: u16 },
    String,
    DateTime,
    Json,
    List,
    FixedBytes { length: u64 },
    Other,
}

fn validate_field_contract(
    field: &FieldDef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    owner: &str,
) -> Result<()> {
    let label = format!("{owner}.{}", field.wire_name);
    if field.required && field.default.is_some() {
        bail!("field `{label}` cannot be required and have a default");
    }
    if !field.required && field.default.is_none() && !matches!(field.ty, TypeRef::Option { .. }) {
        bail!("field `{label}` is optional but has neither an Option type nor an explicit default");
    }
    if let (Some(minimum), Some(maximum)) =
        (field.constraints.min_length, field.constraints.max_length)
        && minimum > maximum
    {
        bail!("field `{label}` has minLength greater than maxLength");
    }
    if let (Some(minimum), Some(maximum)) = (field.constraints.ge, field.constraints.le)
        && minimum > maximum
    {
        bail!("field `{label}` has ge greater than le");
    }

    let kind = field_kind(&field.ty, type_definitions, &mut BTreeSet::new());
    if (field.constraints.min_length.is_some() || field.constraints.max_length.is_some())
        && !matches!(
            kind,
            FieldKind::String | FieldKind::List | FieldKind::FixedBytes { .. }
        )
    {
        bail!("field `{label}` applies length constraints to a non-string/list/bytes type");
    }
    if let FieldKind::FixedBytes { length } = kind
        && (field
            .constraints
            .min_length
            .is_some_and(|minimum| minimum > length)
            || field
                .constraints
                .max_length
                .is_some_and(|maximum| maximum < length))
    {
        bail!(
            "field `{label}` has length constraints incompatible with its fixed byte length {length}"
        );
    }
    if (field.constraints.ge.is_some() || field.constraints.le.is_some())
        && !matches!(kind, FieldKind::Integer { .. })
    {
        bail!("field `{label}` applies ge or le to a non-integer type");
    }
    for (name, value) in [
        ("literal", field.constraints.literal.as_ref()),
        ("default", field.default.as_ref()),
    ] {
        if let Some(value) = value {
            validate_scalar(value, kind, &label, name)?;
            validate_scalar_semantics(
                value,
                &field.ty,
                type_definitions,
                &mut BTreeSet::new(),
                &label,
                name,
            )?;
            validate_scalar_constraints(value, field, &label, name)?;
        }
    }
    if let (Some(literal), Some(default)) = (&field.constraints.literal, &field.default)
        && literal != default
    {
        bail!("field `{label}` has a default that differs from its literal constraint");
    }
    Ok(())
}

fn field_kind(
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    visited: &mut BTreeSet<DefinitionId>,
) -> FieldKind {
    match reference {
        TypeRef::Bool => FieldKind::Bool,
        TypeRef::Int { signed, bits } => FieldKind::Integer {
            signed: *signed,
            bits: *bits,
        },
        TypeRef::String => FieldKind::String,
        TypeRef::DateTime => FieldKind::DateTime,
        TypeRef::Json => FieldKind::Json,
        TypeRef::List { .. } => FieldKind::List,
        TypeRef::FixedBytes { length } => FieldKind::FixedBytes { length: *length },
        TypeRef::Option { item } => field_kind(item, type_definitions, visited),
        TypeRef::Named { identity } if visited.insert(identity.clone()) => {
            match type_definitions.get(identity).map(|item| &item.shape) {
                Some(TypeShape::Alias { target }) => field_kind(target, type_definitions, visited),
                Some(TypeShape::StringEnum { .. }) => FieldKind::String,
                _ => FieldKind::Other,
            }
        }
        _ => FieldKind::Other,
    }
}

fn validate_scalar(value: &ScalarValue, kind: FieldKind, field: &str, name: &str) -> Result<()> {
    let valid = match (value, kind) {
        (ScalarValue::Bool(_), FieldKind::Bool | FieldKind::Json) => true,
        (ScalarValue::String(_), FieldKind::String | FieldKind::DateTime | FieldKind::Json) => true,
        (ScalarValue::I64(value), FieldKind::Integer { signed, bits }) => {
            integer_fits(*value, signed, bits)
        }
        (ScalarValue::I64(value), FieldKind::Json) => {
            *value >= -(JSON_MAX_SAFE_INTEGER as i64) && *value <= JSON_MAX_SAFE_INTEGER as i64
        }
        _ => false,
    };
    if !valid {
        bail!("field `{field}` has a {name} incompatible with its type");
    }
    Ok(())
}

fn validate_scalar_semantics(
    value: &ScalarValue,
    reference: &TypeRef,
    type_definitions: &BTreeMap<DefinitionId, &TypeDef>,
    visited: &mut BTreeSet<DefinitionId>,
    field: &str,
    name: &str,
) -> Result<()> {
    match reference {
        TypeRef::DateTime => {
            let ScalarValue::String(value) = value else {
                return Ok(());
            };
            validate_datetime(value, &format!("field `{field}` {name}"))
        }
        TypeRef::Option { item } => {
            validate_scalar_semantics(value, item, type_definitions, visited, field, name)
        }
        TypeRef::Named { identity } if visited.insert(identity.clone()) => {
            let result = match type_definitions.get(identity).map(|item| &item.shape) {
                Some(TypeShape::Alias { target }) => {
                    validate_scalar_semantics(value, target, type_definitions, visited, field, name)
                }
                Some(TypeShape::StringEnum { variants }) => match value {
                    ScalarValue::String(value)
                        if variants.iter().any(|variant| variant.wire_name == *value) =>
                    {
                        Ok(())
                    }
                    ScalarValue::String(value) => bail!(
                        "field `{field}` has a {name} that is not a declared string enum variant: {value:?}"
                    ),
                    _ => Ok(()),
                },
                _ => Ok(()),
            };
            visited.remove(identity);
            result
        }
        _ => Ok(()),
    }
}

fn validate_datetime(value: &str, path: &str) -> Result<()> {
    rspyts::codec::encode(value, &TypeRef::DateTime, &[])
        .map(|_| ())
        .map_err(|_| anyhow::anyhow!("{path} expected an aware RFC3339 datetime, found {value:?}"))
}

fn integer_fits(value: i64, signed: bool, bits: u16) -> bool {
    if signed {
        match bits {
            8 => i8::try_from(value).is_ok(),
            16 => i16::try_from(value).is_ok(),
            32 => i32::try_from(value).is_ok(),
            64 => true,
            _ => false,
        }
    } else {
        match bits {
            8 => u8::try_from(value).is_ok(),
            16 => u16::try_from(value).is_ok(),
            32 => u32::try_from(value).is_ok(),
            64 => u64::try_from(value).is_ok(),
            _ => false,
        }
    }
}

fn validate_scalar_constraints(
    value: &ScalarValue,
    field: &FieldDef,
    label: &str,
    name: &str,
) -> Result<()> {
    if let ScalarValue::String(value) = value {
        let length = value.chars().count() as u64;
        if field
            .constraints
            .min_length
            .is_some_and(|minimum| length < minimum)
            || field
                .constraints
                .max_length
                .is_some_and(|maximum| length > maximum)
        {
            bail!("field `{label}` has a {name} outside its length constraints");
        }
    }
    if let ScalarValue::I64(value) = value
        && field.constraints.ge.is_some_and(|minimum| *value < minimum)
    {
        bail!("field `{label}` has a {name} below its ge constraint");
    }
    if let ScalarValue::I64(value) = value
        && field.constraints.le.is_some_and(|maximum| *value > maximum)
    {
        bail!("field `{label}` has a {name} above its le constraint");
    }
    Ok(())
}

fn validate_function(
    function: &rspyts::ir::FunctionDef,
    type_ids: &BTreeSet<DefinitionId>,
    error_ids: &BTreeSet<DefinitionId>,
) -> Result<()> {
    validate_name(&function.host_name, "function")?;
    unique(
        "function parameter host name",
        function.params.iter().map(|param| param.host_name.as_str()),
    )?;
    for param in &function.params {
        validate_name(&param.host_name, "function parameter")?;
        validate_ref(&param.ty, type_ids)?;
    }
    validate_ref(&function.returns, type_ids)?;
    validate_error_ref(function.error.as_ref(), error_ids)
}

fn validate_error_ref(
    error: Option<&DefinitionId>,
    error_ids: &BTreeSet<DefinitionId>,
) -> Result<()> {
    if let Some(error) = error
        && !error_ids.contains(error)
    {
        bail!("unresolved error type `{error}`");
    }
    Ok(())
}

fn validate_ref(reference: &TypeRef, type_ids: &BTreeSet<DefinitionId>) -> Result<()> {
    match reference {
        TypeRef::Named { identity } if !type_ids.contains(identity) => {
            bail!("unresolved contract type `{identity}`")
        }
        TypeRef::Option { item } | TypeRef::List { item } => validate_ref(item, type_ids),
        TypeRef::Map { value } => validate_ref(value, type_ids),
        TypeRef::Tuple { items } => {
            for item in items {
                validate_ref(item, type_ids)?;
            }
            Ok(())
        }
        TypeRef::Int { bits, .. } if !matches!(bits, 8 | 16 | 32 | 64) => {
            bail!("unsupported integer width {bits}")
        }
        TypeRef::Float { bits } if !matches!(bits, 32 | 64) => {
            bail!("unsupported float width {bits}")
        }
        _ => Ok(()),
    }
}

fn reject_cycles(types: &[&TypeDef]) -> Result<()> {
    let graph = types
        .iter()
        .map(|item| (type_identity(item), type_dependencies(item)))
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();
    let mut active = Vec::new();
    for id in graph.keys() {
        visit(id, &graph, &mut visited, &mut active)?;
    }
    Ok(())
}

fn visit(
    id: &DefinitionId,
    graph: &BTreeMap<DefinitionId, BTreeSet<DefinitionId>>,
    visited: &mut BTreeSet<DefinitionId>,
    active: &mut Vec<DefinitionId>,
) -> Result<()> {
    if let Some(position) = active.iter().position(|current| current == id) {
        let mut cycle = active[position..].to_vec();
        cycle.push(id.clone());
        bail!(
            "contract type cycle: {}",
            cycle
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" -> ")
        );
    }
    if !visited.insert(id.clone()) {
        return Ok(());
    }
    active.push(id.clone());
    if let Some(dependencies) = graph.get(id) {
        for dependency in dependencies {
            visit(dependency, graph, visited, active)?;
        }
    }
    active.pop();
    Ok(())
}

fn type_dependencies(item: &TypeDef) -> BTreeSet<DefinitionId> {
    let mut dependencies = BTreeSet::new();
    match &item.shape {
        TypeShape::Struct { fields } => {
            for field in fields {
                collect_named(&field.ty, &mut dependencies);
            }
        }
        TypeShape::TaggedEnum { variants, .. } | TypeShape::StringEnum { variants } => {
            for variant in variants {
                for field in &variant.fields {
                    collect_named(&field.ty, &mut dependencies);
                }
            }
        }
        TypeShape::Alias { target } => collect_named(target, &mut dependencies),
    }
    dependencies
}

fn collect_named(reference: &TypeRef, output: &mut BTreeSet<DefinitionId>) {
    match reference {
        TypeRef::Named { identity } => {
            output.insert(identity.clone());
        }
        TypeRef::Option { item } | TypeRef::List { item } => collect_named(item, output),
        TypeRef::Map { value } => collect_named(value, output),
        TypeRef::Tuple { items } => {
            for item in items {
                collect_named(item, output);
            }
        }
        _ => {}
    }
}

fn unique<'a>(kind: &str, values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            bail!("duplicate {kind} `{value}`");
        }
    }
    Ok(())
}

fn unique_owned<T>(kind: &str, values: impl Iterator<Item = T>) -> Result<()>
where
    T: Ord + std::fmt::Display,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            bail!("duplicate {kind}");
        }
    }
    Ok(())
}

fn type_identity(item: &TypeDef) -> DefinitionId {
    DefinitionId {
        owner: item.owner.clone(),
        id: item.id.clone(),
    }
}

fn error_identity(item: &rspyts::ir::ErrorDef) -> DefinitionId {
    DefinitionId {
        owner: item.owner.clone(),
        id: item.id.clone(),
    }
}

fn validate_name(value: &str, kind: &str) -> Result<()> {
    if !identifier(value) {
        bail!("{kind} name `{value}` is not a host identifier");
    }
    Ok(())
}

fn identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rspyts::ir::*;

    use super::*;

    fn empty() -> Manifest {
        Manifest {
            ir_version: IR_VERSION,
            crate_name: "sample".into(),
            crate_version: "1.0.0".into(),
            module_name: "sample".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        }
    }

    fn typescript_resolved(manifest: Manifest, mode: TypeScriptMode) -> ResolvedContract {
        ResolvedContract {
            manifest,
            dependencies: BTreeMap::new(),
            hosts: crate::LockedHosts {
                python: None,
                typescript: Some(crate::LockedTypeScriptHost {
                    package: "sample".into(),
                    mode,
                }),
            },
            foreign_types: BTreeMap::new(),
            foreign_errors: BTreeMap::new(),
        }
    }

    fn typescript_function(name: &str, params: Vec<ParamDef>) -> FunctionDef {
        FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: name.into(),
            host_name: name.into(),
            docs: None,
            target: Target::Typescript,
            params,
            returns: TypeRef::Unit,
            error: None,
        }
    }

    fn typescript_param(name: &str) -> ParamDef {
        ParamDef {
            rust_name: name.into(),
            host_name: name.into(),
            ty: TypeRef::String,
        }
    }

    fn typescript_resource() -> ResourceDef {
        ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Reader".into(),
            name: "Reader".into(),
            docs: None,
            target: Target::Typescript,
            constructors: vec![FunctionDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "new".into(),
                host_name: "new".into(),
                docs: None,
                target: Target::Typescript,
                params: vec![],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Reader"),
                },
                error: None,
            }],
            methods: vec![],
        }
    }

    fn typescript_error(contract: Manifest, mode: TypeScriptMode) -> String {
        if let Err(error) = manifest(&contract) {
            return error.to_string();
        }
        typescript_contract(&typescript_resolved(contract, mode), mode)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn validates_wasm_public_and_ambient_names_without_reserving_private_helpers() {
        for name in [
            "init",
            "globalThis",
            "CONTRACT_FINGERPRINT",
            "ResourceClosedError",
        ] {
            let mut contract = empty();
            contract
                .functions
                .push(typescript_function(name, Vec::new()));
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && (error.contains("generated") || error.contains("ambient")),
                "unexpected error for {name}: {error}"
            );
        }

        for name in [
            "deepFreeze",
            "undefined",
            "initializeNative",
            "native",
            "translateError",
            "initializationPromise",
        ] {
            let mut contract = empty();
            contract
                .functions
                .push(typescript_function(name, Vec::new()));
            manifest(&contract).unwrap();
            typescript_contract(
                &typescript_resolved(contract, TypeScriptMode::Wasm),
                TypeScriptMode::Wasm,
            )
            .unwrap();
        }

        for name in [
            "InitInput",
            "InitOutput",
            "ResourceClosedError",
            "RequestInfo",
            "URL",
            "Response",
            "BufferSource",
            "WebAssembly",
            "Promise",
        ] {
            let mut contract = empty();
            contract.types.push(TypeDef {
                owner: CargoPackageId::new("sample"),
                id: format!("sample::{name}"),
                name: name.into(),
                docs: None,
                shape: TypeShape::Struct { fields: vec![] },
            });
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && (error.contains("generated") || error.contains("ambient")),
                "unexpected error for {name}: {error}"
            );
        }

        let mut contract = empty();
        contract
            .functions
            .push(typescript_function("String", Vec::new()));
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();

        let mut contract = empty();
        let mut resource = typescript_resource();
        resource.name = "Symbol".into();
        resource.id = "sample::Symbol".into();
        resource.constructors[0].returns = TypeRef::Named {
            identity: DefinitionId::new("sample", "sample::Symbol"),
        };
        contract.resources.push(resource);
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();
    }

    #[test]
    fn validates_typescript_bindings_and_member_names_in_their_actual_positions() {
        for name in ["delete", "new", "default", "await", "eval", "arguments"] {
            let mut contract = empty();
            contract
                .functions
                .push(typescript_function(name, Vec::new()));
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && error.contains("reserved"),
                "unexpected export error for {name}: {error}"
            );

            let mut contract = empty();
            contract
                .functions
                .push(typescript_function("run", vec![typescript_param(name)]));
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && error.contains("reserved"),
                "unexpected parameter error for {name}: {error}"
            );

            let mut contract = empty();
            contract.types.push(TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "sample::State".into(),
                name: "State".into(),
                docs: None,
                shape: TypeShape::StringEnum {
                    variants: vec![EnumVariantDef {
                        rust_name: name.into(),
                        wire_name: "value".into(),
                        docs: None,
                        fields: vec![],
                    }],
                },
            });
            manifest(&contract).unwrap();
            typescript_contract(
                &typescript_resolved(contract, TypeScriptMode::Wasm),
                TypeScriptMode::Wasm,
            )
            .unwrap();

            let mut contract = empty();
            let mut resource = typescript_resource();
            resource.methods.push(MethodDef {
                rust_name: name.into(),
                host_name: name.into(),
                docs: None,
                target: Target::Typescript,
                mutable: false,
                params: vec![],
                returns: TypeRef::Unit,
                error: None,
            });
            contract.resources.push(resource);
            manifest(&contract).unwrap();
            typescript_contract(
                &typescript_resolved(contract, TypeScriptMode::Wasm),
                TypeScriptMode::Wasm,
            )
            .unwrap();
        }
    }

    #[test]
    fn reserves_internal_emitter_prefixes() {
        let mut contract = empty();
        contract
            .functions
            .push(typescript_function("__rspyts_internal", Vec::new()));
        let error = typescript_error(contract, TypeScriptMode::Wasm);
        assert!(
            error.contains("__rspyts_internal") && error.contains("reserved rspyts prefix"),
            "unexpected TypeScript export error: {error}"
        );

        let mut contract = empty();
        let mut resource = typescript_resource();
        resource.methods.push(MethodDef {
            rust_name: "read".into(),
            host_name: "__rspyts_internal".into(),
            docs: None,
            target: Target::Typescript,
            mutable: false,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        });
        contract.resources.push(resource);
        let error = typescript_error(contract, TypeScriptMode::Wasm);
        assert!(
            error.contains("__rspyts_internal") && error.contains("reserved rspyts prefix"),
            "unexpected TypeScript member error: {error}"
        );

        let mut contract = empty();
        contract.functions.push(FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "__rspyts_internal".into(),
            host_name: "callInternal".into(),
            docs: None,
            target: Target::Python,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        });
        let error = python_error(&contract);
        assert!(
            error.contains("__rspyts_internal") && error.contains("reserved rspyts prefix"),
            "unexpected Python export error: {error}"
        );

        contract.functions[0].rust_name = "call_internal".into();
        contract.functions[0].params.push(ParamDef {
            rust_name: "__rspyts_internal".into(),
            host_name: "internal".into(),
            ty: TypeRef::String,
        });
        let error = python_error(&contract);
        assert!(
            error.contains("__rspyts_internal") && error.contains("reserved rspyts prefix"),
            "unexpected Python parameter error: {error}"
        );
    }

    #[test]
    fn wasm_wire_name_validation_uses_the_emitted_constant_subset() {
        let unsafe_constant = ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "UNSAFE_WIDE".into(),
            host_name: "CONTRACT_FINGERPRINT".into(),
            docs: None,
            target: Target::Static,
            ty: TypeRef::Int {
                signed: false,
                bits: 64,
            },
            value: serde_json::json!(9_007_199_254_740_992_u64),
        };

        let mut contract = empty();
        contract.constants.push(unsafe_constant.clone());
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();

        let mut contract = empty();
        contract.constants.push(ConstantDef {
            value: serde_json::json!(9_007_199_254_740_991_u64),
            ..unsafe_constant
        });
        let error = typescript_error(contract, TypeScriptMode::Wasm);
        assert!(
            error.contains("CONTRACT_FINGERPRINT") && error.contains("generated constant"),
            "unexpected wire error: {error}"
        );
    }

    #[test]
    fn validates_typescript_resource_member_namespaces() {
        for name in ["constructor", "free"] {
            let mut contract = empty();
            let mut resource = typescript_resource();
            resource.methods.push(MethodDef {
                rust_name: "read".into(),
                host_name: name.into(),
                docs: None,
                target: Target::Typescript,
                mutable: false,
                params: vec![],
                returns: TypeRef::Unit,
                error: None,
            });
            contract.resources.push(resource);
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && error.contains("generated"),
                "unexpected member error for {name}: {error}"
            );
        }

        for name in ["handle", "requireHandle"] {
            let mut contract = empty();
            let mut resource = typescript_resource();
            resource.methods.push(MethodDef {
                rust_name: "read".into(),
                host_name: name.into(),
                docs: None,
                target: Target::Typescript,
                mutable: false,
                params: vec![],
                returns: TypeRef::Unit,
                error: None,
            });
            contract.resources.push(resource);
            manifest(&contract).unwrap();
            typescript_contract(
                &typescript_resolved(contract, TypeScriptMode::Wasm),
                TypeScriptMode::Wasm,
            )
            .unwrap();
        }

        for name in ["constructor", "prototype"] {
            let mut contract = empty();
            let mut resource = typescript_resource();
            resource.constructors.push(FunctionDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "from_value".into(),
                host_name: name.into(),
                docs: None,
                target: Target::Typescript,
                params: vec![],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Reader"),
                },
                error: None,
            });
            contract.resources.push(resource);
            let error = typescript_error(contract, TypeScriptMode::Wasm);
            assert!(
                error.contains(name) && error.contains("generated"),
                "unexpected error for {name}: {error}"
            );
        }

        let mut contract = empty();
        let mut resource = typescript_resource();
        resource.constructors.push(FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "from_value".into(),
            host_name: "close".into(),
            docs: None,
            target: Target::Typescript,
            params: vec![],
            returns: TypeRef::Named {
                identity: DefinitionId::new("sample", "sample::Reader"),
            },
            error: None,
        });
        resource.methods.push(MethodDef {
            rust_name: "close".into(),
            host_name: "close".into(),
            docs: None,
            target: Target::Typescript,
            mutable: false,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        });
        contract.resources.push(resource);
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();
    }

    #[test]
    fn validates_static_and_wasm_wire_names_in_their_own_namespaces() {
        let constant = |target, name: &str| ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: name.into(),
            host_name: name.into(),
            docs: None,
            target,
            ty: TypeRef::String,
            value: serde_json::json!("value"),
        };

        let mut contract = empty();
        contract
            .constants
            .push(constant(Target::Static, "CONTRACT_FINGERPRINT"));
        let error = typescript_error(contract.clone(), TypeScriptMode::Static);
        assert!(
            error.contains("static package") && error.contains("CONTRACT_FINGERPRINT"),
            "unexpected static error: {error}"
        );
        let error = typescript_error(contract, TypeScriptMode::Wasm);
        assert!(
            error.contains("WASM `./wire` package") && error.contains("CONTRACT_FINGERPRINT"),
            "unexpected wire error: {error}"
        );

        for name in ["delete", "new", "default", "await", "eval", "arguments"] {
            let mut contract = empty();
            contract.constants.push(constant(Target::Static, name));
            let error = typescript_error(contract, TypeScriptMode::Static);
            assert!(
                error.contains(name) && error.contains("reserved"),
                "unexpected static binding error for {name}: {error}"
            );
        }

        let mut contract = empty();
        contract
            .functions
            .push(typescript_function("shared", Vec::new()));
        contract.constants.push(constant(Target::Static, "shared"));
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();
    }

    #[test]
    fn allows_type_and_value_exports_with_the_same_typescript_name() {
        let item = || TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Collision".into(),
            name: "Collision".into(),
            docs: None,
            shape: TypeShape::Struct { fields: vec![] },
        };
        let mut contract = empty();
        contract.types.push(item());
        contract
            .functions
            .push(typescript_function("Collision", Vec::new()));
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Wasm),
            TypeScriptMode::Wasm,
        )
        .unwrap();

        let mut contract = empty();
        contract.types.push(item());
        contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "Collision".into(),
            host_name: "Collision".into(),
            docs: None,
            target: Target::Static,
            ty: TypeRef::String,
            value: serde_json::json!("value"),
        });
        manifest(&contract).unwrap();
        typescript_contract(
            &typescript_resolved(contract, TypeScriptMode::Static),
            TypeScriptMode::Static,
        )
        .unwrap();
    }

    #[test]
    fn imported_string_enums_share_the_typescript_runtime_namespace() {
        let imported = TypeDef {
            owner: CargoPackageId::new("dependency"),
            id: "dependency::Status".into(),
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
        let mut contract = empty();
        contract
            .functions
            .push(typescript_function("Status", Vec::new()));
        manifest(&contract).unwrap();
        let mut resolved = typescript_resolved(contract, TypeScriptMode::Wasm);
        resolved
            .foreign_types
            .insert(type_identity(&imported), imported);
        let error = typescript_contract(&resolved, TypeScriptMode::Wasm)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Status")
                && error.contains("imported string enum")
                && error.contains("function"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn typescript_helper_names_do_not_affect_python_only_output() {
        let mut contract = empty();
        let mut function = typescript_function("deepFreeze", Vec::new());
        function.target = Target::Python;
        contract.functions.push(function);
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();
    }

    #[test]
    fn rejects_unresolved_named_types() {
        let mut contract = empty();
        contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "VALUE".into(),
            host_name: "VALUE".into(),
            docs: None,
            target: Target::Static,
            ty: TypeRef::Named {
                identity: DefinitionId::new("sample", "missing"),
            },
            value: serde_json::Value::Null,
        });
        assert!(
            manifest(&contract)
                .unwrap_err()
                .to_string()
                .contains("unresolved")
        );
    }

    #[test]
    fn rejects_type_cycles() {
        let mut contract = empty();
        contract.types = vec![
            TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "a".into(),
                name: "A".into(),
                docs: None,
                shape: TypeShape::Alias {
                    target: TypeRef::Named {
                        identity: DefinitionId::new("sample", "b"),
                    },
                },
            },
            TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "b".into(),
                name: "B".into(),
                docs: None,
                shape: TypeShape::Alias {
                    target: TypeRef::Named {
                        identity: DefinitionId::new("sample", "a"),
                    },
                },
            },
        ];
        assert!(
            manifest(&contract)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    fn field(ty: TypeRef) -> FieldDef {
        FieldDef {
            rust_name: "value".into(),
            wire_name: "value".into(),
            docs: None,
            ty,
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        }
    }

    fn contract_with_field(field: FieldDef) -> Manifest {
        let mut contract = empty();
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Item".into(),
            name: "Item".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![field],
            },
        });
        contract
    }

    fn tagged_type(name: &str, variants: Vec<EnumVariantDef>) -> TypeDef {
        TypeDef {
            owner: CargoPackageId::new("sample"),
            id: format!("sample::{name}"),
            name: name.into(),
            docs: None,
            shape: TypeShape::TaggedEnum {
                tag: "kind".into(),
                variants,
            },
        }
    }

    fn variant(name: &str, wire_name: &str, fields: Vec<FieldDef>) -> EnumVariantDef {
        EnumVariantDef {
            rust_name: name.into(),
            wire_name: wire_name.into(),
            docs: None,
            fields,
        }
    }

    fn python_resource(name: &str) -> ResourceDef {
        ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: format!("sample::{name}"),
            name: name.into(),
            docs: None,
            target: Target::Python,
            constructors: vec![FunctionDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "new".into(),
                host_name: "new".into(),
                docs: None,
                target: Target::Python,
                params: vec![],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", format!("sample::{name}")),
                },
                error: None,
            }],
            methods: vec![],
        }
    }

    fn python_error(contract: &Manifest) -> String {
        manifest(contract).unwrap();
        python_manifest(contract).unwrap_err().to_string()
    }

    #[test]
    fn rejects_tagged_variant_fields_that_duplicate_the_wire_discriminator() {
        let mut contract = empty();
        let mut duplicate = field(TypeRef::String);
        duplicate.rust_name = "kind_value".into();
        duplicate.wire_name = "kind".into();
        contract.types.push(tagged_type(
            "Event",
            vec![variant("Data", "data", vec![duplicate])],
        ));
        let error = manifest(&contract).unwrap_err();
        assert_eq!(
            error.to_string(),
            "tagged enum `Event::Data` field wire name `kind` duplicates its discriminator tag"
        );

        let TypeShape::TaggedEnum { variants, .. } = &mut contract.types[0].shape else {
            unreachable!();
        };
        variants[0].fields[0].wire_name = "kindValue".into();
        manifest(&contract).unwrap();
    }

    #[test]
    fn rejects_colliding_synthesized_python_variant_classes() {
        let mut contract = empty();
        contract.types = vec![
            TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "sample::EventData".into(),
                name: "EventData".into(),
                docs: None,
                shape: TypeShape::Struct { fields: vec![] },
            },
            tagged_type("Event", vec![variant("Data", "data", vec![])]),
        ];
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `EventData`")
                && error.contains("type `EventData`")
                && error.contains("type `Event` variant `Data` generated class"),
            "unexpected error: {error}"
        );

        contract.types = vec![tagged_type("Event", vec![variant("Data", "data", vec![])])];
        contract.errors = vec![ErrorDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::EventDataError".into(),
            name: "EventData".into(),
            docs: None,
            variants: vec![],
        }];
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `EventData`")
                && error.contains("error `EventData`")
                && error.contains("type `Event` variant `Data` generated class"),
            "unexpected error: {error}"
        );
        contract.errors.clear();

        contract.types = vec![tagged_type(
            "Event",
            vec![
                variant("FooBar", "fooBar", vec![]),
                variant("foo_bar", "foo_bar", vec![]),
            ],
        )];
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `EventFooBar`")
                && error.contains("variant `FooBar`")
                && error.contains("variant `foo_bar`"),
            "unexpected error: {error}"
        );

        contract.types = vec![
            tagged_type("AB", vec![variant("C", "c", vec![])]),
            tagged_type("A", vec![variant("BC", "bc", vec![])]),
        ];
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `ABC`")
                && error.contains("type `AB` variant `C`")
                && error.contains("type `A` variant `BC`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_unsafe_python_names_before_emission() {
        for (name, expected) in [
            ("class", "not a public identifier"),
            ("_private", "not a public identifier"),
            ("model_config", "reserved by Pydantic"),
            ("model_fields", "reserved by Pydantic"),
            ("model_extra", "reserved by Pydantic"),
            ("model_computed_fields", "reserved by Pydantic"),
            ("model_dump", "reserved by Pydantic"),
            ("model_validate_json", "reserved by Pydantic"),
        ] {
            let mut invalid = field(TypeRef::String);
            invalid.rust_name = name.into();
            invalid.wire_name = "safeWireName".into();
            let contract = contract_with_field(invalid);
            let error = python_error(&contract);
            assert!(
                error.contains(expected) && error.contains("field `safeWireName`"),
                "unexpected error for {name}: {error}"
            );
        }

        let mut contract = empty();
        contract.functions.push(FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "class".into(),
            host_name: "callClass".into(),
            docs: None,
            target: Target::Python,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        });
        let error = python_error(&contract);
        assert!(
            error.contains("function `class`") && error.contains("not an identifier"),
            "unexpected error: {error}"
        );

        contract.functions[0].rust_name = "safe_call".into();
        contract.functions[0].params.push(ParamDef {
            rust_name: "_value".into(),
            host_name: "value".into(),
            ty: TypeRef::String,
        });
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();

        contract.functions.clear();
        contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "VALUE".into(),
            host_name: "class".into(),
            docs: None,
            target: Target::Python,
            ty: TypeRef::String,
            value: serde_json::json!("value"),
        });
        let error = python_error(&contract);
        assert!(
            error.contains("constant `class`") && error.contains("not an identifier"),
            "unexpected error: {error}"
        );

        for name in [
            "bool",
            "int",
            "float",
            "list",
            "enumerate",
            "dict",
            "bytes",
            "super",
            "classmethod",
            "Field",
            "_private",
        ] {
            let mut contract = empty();
            contract.types.push(TypeDef {
                owner: CargoPackageId::new("sample"),
                id: format!("sample::{name}"),
                name: name.into(),
                docs: None,
                shape: TypeShape::Struct { fields: vec![] },
            });
            manifest(&contract).unwrap();
            python_manifest(&contract).unwrap();
        }

        let mut model_value = field(TypeRef::String);
        model_value.rust_name = "model_value".into();
        manifest(&contract_with_field(model_value.clone())).unwrap();
        python_manifest(&contract_with_field(model_value)).unwrap();

        let mut field_binding = field(TypeRef::String);
        field_binding.rust_name = "Field".into();
        manifest(&contract_with_field(field_binding.clone())).unwrap();
        python_manifest(&contract_with_field(field_binding)).unwrap();
    }

    #[test]
    fn rejects_colliding_native_python_symbols() {
        let function = |rust_name: &str, host_name: &str| FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: rust_name.into(),
            host_name: host_name.into(),
            docs: None,
            target: Target::Python,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        };

        let mut contract = empty();
        contract
            .functions
            .push(function("buffer_payload", "BufferPayload"));
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `BufferPayload`")
                && error.contains("generated native buffer class")
                && error.contains("function `buffer_payload` native host"),
            "unexpected error: {error}"
        );

        contract.functions.clear();
        contract.resources.push(python_resource("BufferPayload"));
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `BufferPayload`")
                && error.contains("generated native buffer class")
                && error.contains("resource `BufferPayload` native class"),
            "unexpected error: {error}"
        );

        contract.resources = vec![python_resource("Reader")];
        contract.functions.push(function("open_reader", "Reader"));
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `Reader`")
                && error.contains("function `open_reader` native host")
                && error.contains("resource `Reader` native class"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_exports_that_collide_with_generated_python_modules() {
        for name in [
            "models",
            "codecs",
            "errors",
            "functions",
            "resources",
            "constants",
        ] {
            let mut contract = empty();
            contract.types.push(TypeDef {
                owner: CargoPackageId::new("sample"),
                id: format!("sample::{name}"),
                name: name.into(),
                docs: None,
                shape: TypeShape::Struct { fields: vec![] },
            });
            let error = python_error(&contract);
            assert!(
                error.contains(&format!("Python name `{name}`"))
                    && error.contains(&format!("generated Python module `{name}`")),
                "unexpected error for {name}: {error}"
            );
        }

        let mut contract = empty();
        contract.module_name = "bridge".into();
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::bridge".into(),
            name: "bridge".into(),
            docs: None,
            shape: TypeShape::Struct { fields: vec![] },
        });
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `bridge`")
                && error.contains("generated Python module `bridge`")
                && error.contains("type `bridge`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_unsafe_python_resource_names_and_injected_locals() {
        let mut contract = empty();
        contract.resources.push(ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Reader".into(),
            name: "Reader".into(),
            docs: None,
            target: Target::Python,
            constructors: vec![FunctionDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "new".into(),
                host_name: "new".into(),
                docs: None,
                target: Target::Python,
                params: vec![ParamDef {
                    rust_name: "self".into(),
                    host_name: "self".into(),
                    ty: TypeRef::String,
                }],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Reader"),
                },
                error: None,
            }],
            methods: vec![],
        });
        let error = python_error(&contract);
        assert!(
            error.contains("parameter `self`") && error.contains("generated resource local"),
            "unexpected error: {error}"
        );

        contract.resources[0].constructors[0].params.clear();
        contract.resources[0].methods.push(MethodDef {
            rust_name: "handle".into(),
            host_name: "handle".into(),
            docs: None,
            target: Target::Python,
            mutable: false,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        });
        let error = python_error(&contract);
        assert!(
            error.contains("method `handle`") && error.contains("resource wrapper"),
            "unexpected error: {error}"
        );

        contract.resources[0].methods[0].rust_name = "close".into();
        contract.resources[0].methods[0].host_name = "close".into();
        let error = manifest(&contract).unwrap_err();
        assert!(
            error.to_string().contains(
                "Python native resource `Reader` member `close` from method `close` collides with generated Python native resource member `close`"
            ),
            "unexpected error: {error}"
        );
        let error = python_manifest(&contract).unwrap_err();
        assert!(
            error.to_string().contains("method `close`")
                && error
                    .to_string()
                    .contains("reserved by the resource wrapper"),
            "unexpected error: {error}"
        );

        contract.resources[0].methods[0].rust_name = "free".into();
        contract.resources[0].methods[0].host_name = "free".into();
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();

        contract.resources[0].methods[0].rust_name = "open_reader".into();
        contract.resources[0].methods[0].host_name = "read".into();
        contract.resources[0].constructors.push(FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "open_reader".into(),
            host_name: "openReader".into(),
            docs: None,
            target: Target::Python,
            params: vec![],
            returns: TypeRef::Named {
                identity: DefinitionId::new("sample", "sample::Reader"),
            },
            error: None,
        });
        let error = python_error(&contract);
        assert!(
            error.contains("Python name `open_reader`")
                && error.contains("constructor `open_reader`")
                && error.contains("method `open_reader`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resource_name_validation_matches_the_emitted_python_scopes() {
        let parameter = |rust_name: &str, host_name: &str| ParamDef {
            rust_name: rust_name.into(),
            host_name: host_name.into(),
            ty: TypeRef::String,
        };
        let mut contract = empty();
        contract.resources.push(ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Reader".into(),
            name: "Reader".into(),
            docs: None,
            target: Target::Python,
            constructors: vec![
                FunctionDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "new".into(),
                    host_name: "new".into(),
                    docs: None,
                    target: Target::Python,
                    params: vec![parameter("resource", "class")],
                    returns: TypeRef::Named {
                        identity: DefinitionId::new("sample", "sample::Reader"),
                    },
                    error: None,
                },
                FunctionDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "from_value".into(),
                    host_name: "fromValue".into(),
                    docs: None,
                    target: Target::Python,
                    params: vec![parameter("self", "class")],
                    returns: TypeRef::Named {
                        identity: DefinitionId::new("sample", "sample::Reader"),
                    },
                    error: None,
                },
            ],
            methods: vec![MethodDef {
                rust_name: "read".into(),
                host_name: "read".into(),
                docs: None,
                target: Target::Python,
                mutable: false,
                params: vec![parameter("resource", "class")],
                returns: TypeRef::Unit,
                error: None,
            }],
        });
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();

        contract.resources[0].constructors.truncate(1);
        contract.resources[0].methods.clear();
        for name in ["handle", "close", "free"] {
            contract.resources[0].constructors[0].rust_name = name.into();
            contract.resources[0].constructors[0].host_name = name.into();
            manifest(&contract).unwrap();
            python_manifest(&contract).unwrap();
        }
    }

    #[test]
    fn host_name_uniqueness_is_scoped_to_each_emitter() {
        let function = |rust_name: &str, target| FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: rust_name.into(),
            host_name: "shared".into(),
            docs: None,
            target,
            params: vec![],
            returns: TypeRef::Unit,
            error: None,
        };
        let constant = |rust_name: &str, target| ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: rust_name.into(),
            host_name: "SHARED_VALUE".into(),
            docs: None,
            target,
            ty: TypeRef::String,
            value: serde_json::json!("value"),
        };
        let mut contract = empty();
        contract.functions = vec![
            function("python_action", Target::Python),
            function("typescript_action", Target::Typescript),
        ];
        contract.constants = vec![
            constant("PYTHON_VALUE", Target::Python),
            constant("TYPESCRIPT_VALUE", Target::Typescript),
        ];
        contract.resources.push(ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Reader".into(),
            name: "Reader".into(),
            docs: None,
            target: Target::Both,
            constructors: vec![
                FunctionDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "python_new".into(),
                    host_name: "sharedConstructor".into(),
                    docs: None,
                    target: Target::Python,
                    params: vec![],
                    returns: TypeRef::Named {
                        identity: DefinitionId::new("sample", "sample::Reader"),
                    },
                    error: None,
                },
                FunctionDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "typescript_new".into(),
                    host_name: "sharedConstructor".into(),
                    docs: None,
                    target: Target::Typescript,
                    params: vec![],
                    returns: TypeRef::Named {
                        identity: DefinitionId::new("sample", "sample::Reader"),
                    },
                    error: None,
                },
            ],
            methods: vec![
                MethodDef {
                    rust_name: "python_read".into(),
                    host_name: "sharedMethod".into(),
                    docs: None,
                    target: Target::Python,
                    mutable: false,
                    params: vec![],
                    returns: TypeRef::Unit,
                    error: None,
                },
                MethodDef {
                    rust_name: "typescript_read".into(),
                    host_name: "sharedMethod".into(),
                    docs: None,
                    target: Target::Typescript,
                    mutable: false,
                    params: vec![],
                    returns: TypeRef::Unit,
                    error: None,
                },
            ],
        });
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();

        contract.resources[0].constructors[1].target = Target::Both;
        contract.resources[0].constructors[1].host_name = "sharedMethod".into();
        let error = manifest(&contract).unwrap_err();
        assert!(
            error.to_string().contains(
                "Python native resource `Reader` member `sharedMethod` from method `python_read` collides with constructor `typescript_new`"
            ),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn accepts_hostile_wire_aliases_and_tags_with_safe_python_attributes() {
        let mut contract = empty();
        let mut class_alias = field(TypeRef::String);
        class_alias.rust_name = "class_value".into();
        class_alias.wire_name = "class".into();
        let mut private_alias = field(TypeRef::String);
        private_alias.rust_name = "private_value".into();
        private_alias.wire_name = "_private".into();
        let mut model_alias = field(TypeRef::String);
        model_alias.rust_name = "config_value".into();
        model_alias.wire_name = "model_config".into();
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Payload".into(),
            name: "Payload".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![class_alias, private_alias, model_alias],
            },
        });
        let mut event = tagged_type("Event", vec![variant("Data", "data", vec![])]);
        let TypeShape::TaggedEnum { tag, .. } = &mut event.shape else {
            unreachable!();
        };
        *tag = "event-type".into();
        contract.types.push(event);
        manifest(&contract).unwrap();
        python_manifest(&contract).unwrap();
    }

    #[test]
    fn validates_constraint_and_default_invariants() {
        let mut invalid_lengths = field(TypeRef::String);
        invalid_lengths.constraints.min_length = Some(3);
        invalid_lengths.constraints.max_length = Some(2);
        assert!(
            manifest(&contract_with_field(invalid_lengths))
                .unwrap_err()
                .to_string()
                .contains("minLength")
        );

        let mut invalid_bounds = field(TypeRef::Int {
            signed: true,
            bits: 32,
        });
        invalid_bounds.constraints.ge = Some(3);
        invalid_bounds.constraints.le = Some(2);
        assert!(
            manifest(&contract_with_field(invalid_bounds))
                .unwrap_err()
                .to_string()
                .contains("ge greater than le")
        );

        let mut invalid_default = field(TypeRef::Int {
            signed: true,
            bits: 32,
        });
        invalid_default.required = false;
        invalid_default.default = Some(ScalarValue::I64(0));
        invalid_default.constraints.ge = Some(1);
        assert!(
            manifest(&contract_with_field(invalid_default))
                .unwrap_err()
                .to_string()
                .contains("below its ge")
        );

        let mut datetime = field(TypeRef::DateTime);
        datetime.required = false;
        datetime.default = Some(ScalarValue::String("2026-01-01T00:00:00Z".into()));
        manifest(&contract_with_field(datetime)).unwrap();
    }

    #[test]
    fn fixed_bytes_validate_constant_values_and_field_constraints() {
        let mut contract = empty();
        contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "DIGEST".into(),
            host_name: "DIGEST".into(),
            docs: None,
            target: Target::Both,
            ty: TypeRef::FixedBytes { length: 4 },
            value: serde_json::json!([0, 1, 2, 255]),
        });
        manifest(&contract).unwrap();

        contract.constants[0].value = serde_json::json!([0, 1, 2]);
        let error = manifest(&contract).unwrap_err();
        assert!(
            error.to_string().contains("expected exactly 4 bytes"),
            "unexpected error: {error}"
        );

        contract.constants[0].value = serde_json::json!([0, 1, 2, 256]);
        let error = manifest(&contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expected an unsigned 8-bit integer"),
            "unexpected error: {error}"
        );

        let mut compatible = field(TypeRef::FixedBytes { length: 4 });
        compatible.constraints.min_length = Some(4);
        compatible.constraints.max_length = Some(4);
        manifest(&contract_with_field(compatible)).unwrap();

        let mut incompatible = field(TypeRef::FixedBytes { length: 4 });
        incompatible.constraints.min_length = Some(5);
        let error = manifest(&contract_with_field(incompatible)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("incompatible with its fixed byte length 4"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_non_finite_float_constants_for_every_host_target() {
        for target in [
            Target::Both,
            Target::Python,
            Target::Typescript,
            Target::Static,
        ] {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let mut contract = empty();
                contract.constants.push(ConstantDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "VALUE".into(),
                    host_name: "VALUE".into(),
                    docs: None,
                    target,
                    ty: TypeRef::Float { bits: 64 },
                    value: serde_json::to_value(value).unwrap(),
                });
                let error = manifest(&contract).unwrap_err();
                assert!(
                    error.to_string().contains("expected a finite f64"),
                    "unexpected {target:?} error for {value:?}: {error}"
                );
            }
        }
    }

    #[test]
    fn recursively_validates_typed_constant_values_for_every_host_target() {
        let named = |id: &str| TypeRef::Named {
            identity: DefinitionId::new("sample", id),
        };
        let named_field = |name: &str, ty: TypeRef| FieldDef {
            rust_name: name.into(),
            wire_name: name.into(),
            docs: None,
            ty,
            required: true,
            default: None,
            constraints: FieldConstraints::default(),
        };
        let mut count = named_field(
            "count",
            TypeRef::Int {
                signed: false,
                bits: 16,
            },
        );
        count.constraints.ge = Some(2);
        let definitions = vec![
            TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "sample::Status".into(),
                name: "Status".into(),
                docs: None,
                shape: TypeShape::StringEnum {
                    variants: vec![variant("Ready", "ready", vec![])],
                },
            },
            tagged_type("Event", vec![variant("Data", "data", vec![count])]),
            TypeDef {
                owner: CargoPackageId::new("sample"),
                id: "sample::Payload".into(),
                name: "Payload".into(),
                docs: None,
                shape: TypeShape::Struct {
                    fields: vec![
                        named_field("active", TypeRef::Bool),
                        named_field(
                            "pair",
                            TypeRef::Tuple {
                                items: vec![TypeRef::String, TypeRef::FixedBytes { length: 2 }],
                            },
                        ),
                        named_field(
                            "lookup",
                            TypeRef::Map {
                                value: Box::new(TypeRef::Int {
                                    signed: false,
                                    bits: 16,
                                }),
                            },
                        ),
                        named_field("status", named("sample::Status")),
                        named_field("event", named("sample::Event")),
                        named_field(
                            "samples",
                            TypeRef::Buffer {
                                element: BufferElement::F32,
                            },
                        ),
                        named_field(
                            "note",
                            TypeRef::Option {
                                item: Box::new(TypeRef::String),
                            },
                        ),
                        named_field("created_at", TypeRef::DateTime),
                    ],
                },
            },
        ];
        let valid = serde_json::json!({
            "active": true,
            "pair": ["ok", [1, 255]],
            "lookup": {"first": 7},
            "status": "ready",
            "event": {"kind": "data", "count": 2},
            "samples": [1.25, -2.5],
            "note": null,
            "created_at": "2026-07-17T12:00:00Z"
        });
        let invalid = vec![
            (
                serde_json::json!({
                    "active": null,
                    "pair": ["ok", [1, 255]], "lookup": {"first": 7},
                    "status": "ready", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "expected boolean",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok"], "lookup": {"first": 7},
                    "status": "ready", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "expected tuple with 2 elements",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 256]], "lookup": {"first": 7},
                    "status": "ready", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "unsigned 8-bit integer",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 255]], "lookup": {"first": -1},
                    "status": "ready", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "unsigned 16-bit integer",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 255]], "lookup": {"first": 7},
                    "status": "missing", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "declared string enum variant",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 255]], "lookup": {"first": 7},
                    "status": "ready", "event": {"kind": "data", "count": 1},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "expected integer >= 2",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 255]], "lookup": {"first": 7},
                    "status": "ready", "event": {"count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "2026-07-17T12:00:00Z"
                }),
                "expected required tag",
            ),
            (
                serde_json::json!({
                    "active": true,
                    "pair": ["ok", [1, 255]], "lookup": {"first": 7},
                    "status": "ready", "event": {"kind": "data", "count": 2},
                    "samples": [1.25], "note": null,
                    "created_at": "not-a-datetime"
                }),
                "aware RFC3339 datetime",
            ),
        ];

        for target in [
            Target::Both,
            Target::Python,
            Target::Typescript,
            Target::Static,
        ] {
            let contract = |value| {
                let mut contract = empty();
                contract.types = definitions.clone();
                contract.constants.push(ConstantDef {
                    owner: CargoPackageId::new("sample"),
                    rust_name: "PAYLOAD".into(),
                    host_name: "PAYLOAD".into(),
                    docs: None,
                    target,
                    ty: named("sample::Payload"),
                    value,
                });
                contract
            };
            manifest(&contract(valid.clone())).unwrap();
            for (value, expected) in &invalid {
                let error = manifest(&contract(value.clone())).unwrap_err();
                assert!(
                    error.to_string().contains(expected),
                    "unexpected {target:?} error for {value}: {error}"
                );
            }
        }
    }

    #[test]
    fn alias_to_option_null_constants_skip_non_null_field_constraints() {
        for target in [
            Target::Both,
            Target::Python,
            Target::Typescript,
            Target::Static,
        ] {
            let alias_identity = DefinitionId::new("sample", "sample::OptionalLabel");
            let holder_identity = DefinitionId::new("sample", "sample::Holder");
            let mut label = field(TypeRef::Named {
                identity: alias_identity.clone(),
            });
            label.constraints.min_length = Some(3);
            let mut contract = empty();
            contract.types = vec![
                TypeDef {
                    owner: CargoPackageId::new("sample"),
                    id: alias_identity.id,
                    name: "OptionalLabel".into(),
                    docs: None,
                    shape: TypeShape::Alias {
                        target: TypeRef::Option {
                            item: Box::new(TypeRef::String),
                        },
                    },
                },
                TypeDef {
                    owner: CargoPackageId::new("sample"),
                    id: holder_identity.id.clone(),
                    name: "Holder".into(),
                    docs: None,
                    shape: TypeShape::Struct {
                        fields: vec![label],
                    },
                },
            ];
            contract.constants.push(ConstantDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "HOLDER".into(),
                host_name: "HOLDER".into(),
                docs: None,
                target,
                ty: TypeRef::Named {
                    identity: holder_identity,
                },
                value: serde_json::json!({"value": null}),
            });
            manifest(&contract).unwrap();
        }
    }

    #[test]
    fn rejects_non_injective_options_in_every_contract_position() {
        let mut field_contract = contract_with_field(field(TypeRef::Option {
            item: Box::new(TypeRef::Unit),
        }));
        let error = manifest(&field_contract).unwrap_err();
        assert!(
            error.to_string().contains("type `Item` field `value`")
                && error.to_string().contains("non-injective Option"),
            "unexpected field error: {error}"
        );

        field_contract.types.clear();
        field_contract.functions.push(FunctionDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "consume".into(),
            host_name: "consume".into(),
            docs: None,
            target: Target::Python,
            params: vec![ParamDef {
                rust_name: "value".into(),
                host_name: "value".into(),
                ty: TypeRef::Option {
                    item: Box::new(TypeRef::Json),
                },
            }],
            returns: TypeRef::Unit,
            error: None,
        });
        let error = manifest(&field_contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("function `consume` parameter `value`")
                && error.to_string().contains("non-injective Option"),
            "unexpected parameter error: {error}"
        );

        field_contract.functions[0].params.clear();
        field_contract.functions[0].returns = TypeRef::Option {
            item: Box::new(TypeRef::Option {
                item: Box::new(TypeRef::String),
            }),
        };
        let error = manifest(&field_contract).unwrap_err();
        assert!(
            error.to_string().contains("function `consume` return")
                && error.to_string().contains("non-injective Option"),
            "unexpected return error: {error}"
        );

        let nullable_identity = DefinitionId::new("sample", "sample::Nullable");
        let mut constant_contract = empty();
        constant_contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: nullable_identity.id.clone(),
            name: "Nullable".into(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::Json,
            },
        });
        constant_contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "VALUE".into(),
            host_name: "VALUE".into(),
            docs: None,
            target: Target::Both,
            ty: TypeRef::Option {
                item: Box::new(TypeRef::Named {
                    identity: nullable_identity,
                }),
            },
            value: Value::Null,
        });
        let error = manifest(&constant_contract).unwrap_err();
        assert!(
            error.to_string().contains("constant `VALUE`")
                && error.to_string().contains("non-injective Option"),
            "unexpected constant error: {error}"
        );
    }

    #[test]
    fn validates_named_enum_and_datetime_field_scalars_semantically() {
        let mut contract = empty();
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Status".into(),
            name: "Status".into(),
            docs: None,
            shape: TypeShape::StringEnum {
                variants: vec![variant("Ready", "ready", vec![])],
            },
        });
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::StatusAlias".into(),
            name: "StatusAlias".into(),
            docs: None,
            shape: TypeShape::Alias {
                target: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Status"),
                },
            },
        });
        let mut status = field(TypeRef::Named {
            identity: DefinitionId::new("sample", "sample::StatusAlias"),
        });
        status.required = false;
        status.default = Some(ScalarValue::String("missing".into()));
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Item".into(),
            name: "Item".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![status],
            },
        });
        let error = manifest(&contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not a declared string enum variant"),
            "unexpected error: {error}"
        );

        if let TypeShape::Struct { fields } = &mut contract.types[2].shape {
            fields[0].default = Some(ScalarValue::String("ready".into()));
        }
        manifest(&contract).unwrap();
        if let TypeShape::Struct { fields } = &mut contract.types[2].shape {
            fields[0].required = true;
            fields[0].default = None;
            fields[0].constraints.literal = Some(ScalarValue::String("missing".into()));
        }
        let error = manifest(&contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not a declared string enum variant"),
            "unexpected error: {error}"
        );

        let mut datetime = field(TypeRef::DateTime);
        datetime.required = false;
        datetime.default = Some(ScalarValue::String("2026-07-17".into()));
        let error = manifest(&contract_with_field(datetime)).unwrap_err();
        assert!(
            error.to_string().contains("aware RFC3339 datetime"),
            "unexpected error: {error}"
        );

        let mut datetime = field(TypeRef::DateTime);
        datetime.constraints.literal = Some(ScalarValue::String("still-not-a-datetime".into()));
        let error = manifest(&contract_with_field(datetime)).unwrap_err();
        assert!(
            error.to_string().contains("aware RFC3339 datetime"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn accepts_constant_target_scopes() {
        let mut contract = empty();
        for (name, target) in [
            ("BOTH", Target::Both),
            ("PYTHON", Target::Python),
            ("TYPESCRIPT", Target::Typescript),
            ("STATIC", Target::Static),
        ] {
            contract.constants.push(ConstantDef {
                owner: CargoPackageId::new("sample"),
                rust_name: name.into(),
                host_name: name.into(),
                docs: None,
                target,
                ty: TypeRef::String,
                value: serde_json::Value::String("value".into()),
            });
        }
        manifest(&contract).unwrap();
    }

    #[test]
    fn json_numbers_in_constants_and_field_scalars_are_javascript_safe() {
        let mut contract = empty();
        contract.types.push(TypeDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Payload".into(),
            name: "Payload".into(),
            docs: None,
            shape: TypeShape::Struct {
                fields: vec![FieldDef {
                    rust_name: "metadata".into(),
                    wire_name: "metadata".into(),
                    docs: None,
                    ty: TypeRef::Json,
                    required: true,
                    default: None,
                    constraints: FieldConstraints::default(),
                }],
            },
        });
        contract.constants.push(ConstantDef {
            owner: CargoPackageId::new("sample"),
            rust_name: "PAYLOADS".into(),
            host_name: "PAYLOADS".into(),
            docs: None,
            target: Target::Python,
            ty: TypeRef::List {
                item: Box::new(TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Payload"),
                }),
            },
            value: serde_json::json!([{
                "metadata": {
                    "edges": [-9_007_199_254_740_991_i64, 9_007_199_254_740_991_u64],
                    "fraction": 1.25
                }
            }]),
        });
        manifest(&contract).unwrap();

        contract.constants[0].value =
            serde_json::json!([{"metadata": {"items": [9_007_199_254_740_992_u64]}}]);
        let error = manifest(&contract).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("[0].metadata[\"items\"][0] contains JSON integer"),
            "unexpected error: {error}"
        );

        let mut field = field(TypeRef::Json);
        field.required = false;
        field.default = Some(ScalarValue::I64(9_007_199_254_740_992));
        let error = manifest(&contract_with_field(field)).unwrap_err();
        assert!(
            error.to_string().contains("incompatible"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn requires_constructors_for_every_resource_target() {
        let mut contract = empty();
        contract.resources.push(ResourceDef {
            owner: CargoPackageId::new("sample"),
            id: "sample::Reader".into(),
            name: "Reader".into(),
            docs: None,
            target: Target::Both,
            constructors: vec![FunctionDef {
                owner: CargoPackageId::new("sample"),
                rust_name: "new".into(),
                host_name: "new".into(),
                docs: None,
                target: Target::Python,
                params: vec![],
                returns: TypeRef::Named {
                    identity: DefinitionId::new("sample", "sample::Reader"),
                },
                error: None,
            }],
            methods: vec![],
        });
        assert!(
            manifest(&contract)
                .unwrap_err()
                .to_string()
                .contains("TypeScript constructor")
        );
    }
}
