use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use rspyts::ir::{
    CargoPackageId, DefinitionId, FieldDef, Manifest, ScalarValue, Target, TypeDef, TypeRef,
    TypeShape,
};

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
        "function host name",
        manifest
            .functions
            .iter()
            .map(|item| item.host_name.as_str()),
    )?;
    unique(
        "resource id",
        manifest.resources.iter().map(|item| item.id.as_str()),
    )?;
    unique(
        "resource name",
        manifest.resources.iter().map(|item| item.name.as_str()),
    )?;
    unique(
        "constant host name",
        manifest
            .constants
            .iter()
            .map(|item| item.host_name.as_str()),
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
    unique(
        "Python export name",
        std::iter::once("JsonValue")
            .chain(std::iter::once("ContractError"))
            .chain(std::iter::once("ResourceClosedError"))
            .chain(std::iter::once("CONTRACT_FINGERPRINT"))
            .chain(all_types.iter().map(|item| item.name.as_str()))
            .chain(all_errors.iter().map(|item| item.name.as_str()))
            .chain(
                manifest
                    .functions
                    .iter()
                    .filter(|item| matches!(item.target, Target::Both | Target::Python))
                    .map(|item| item.rust_name.as_str()),
            )
            .chain(
                manifest
                    .resources
                    .iter()
                    .filter(|item| matches!(item.target, Target::Both | Target::Python))
                    .map(|item| item.name.as_str()),
            )
            .chain(
                manifest
                    .constants
                    .iter()
                    .filter(|item| matches!(item.target, Target::Both | Target::Python))
                    .map(|item| item.host_name.as_str()),
            ),
    )?;
    unique(
        "TypeScript export name",
        std::iter::once("JsonValue")
            .chain(std::iter::once("CONTRACT_FINGERPRINT"))
            .chain(all_types.iter().map(|item| item.name.as_str()))
            .chain(all_errors.iter().map(|item| item.name.as_str()))
            .chain(
                manifest
                    .functions
                    .iter()
                    .filter(|item| matches!(item.target, Target::Both | Target::Typescript))
                    .map(|item| item.host_name.as_str()),
            )
            .chain(
                manifest
                    .resources
                    .iter()
                    .filter(|item| matches!(item.target, Target::Both | Target::Typescript))
                    .map(|item| item.name.as_str()),
            )
            .chain(
                manifest
                    .constants
                    .iter()
                    .filter(|item| {
                        matches!(
                            item.target,
                            Target::Both | Target::Typescript | Target::Static
                        )
                    })
                    .map(|item| item.host_name.as_str()),
            ),
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
        unique(
            "resource method host name",
            resource
                .methods
                .iter()
                .map(|method| method.host_name.as_str()),
        )?;
        unique(
            "resource member host name",
            resource
                .constructors
                .iter()
                .map(|constructor| constructor.host_name.as_str())
                .chain(
                    resource
                        .methods
                        .iter()
                        .map(|method| method.host_name.as_str()),
                ),
        )?;
        for constructor in &resource.constructors {
            if constructor.owner != root_owner {
                bail!("resource constructor is owned by a foreign Cargo package");
            }
            validate_resource_lifecycle_name(
                &constructor.rust_name,
                &constructor.host_name,
                &resource.name,
            )?;
            validate_child_target(
                resource.target,
                constructor.target,
                &resource.name,
                "constructor",
            )?;
            validate_function(constructor, &type_ids, &error_ids)?;
        }
        for method in &resource.methods {
            validate_resource_lifecycle_name(&method.rust_name, &method.host_name, &resource.name)?;
            validate_child_target(resource.target, method.target, &resource.name, "method")?;
            validate_name(&method.host_name, "method")?;
            for param in &method.params {
                validate_name(&param.host_name, "method parameter")?;
                validate_ref(&param.ty, &type_ids)?;
            }
            validate_ref(&method.returns, &type_ids)?;
            validate_error_ref(method.error.as_ref(), &error_ids)?;
        }
    }
    for constant in &manifest.constants {
        validate_name(&constant.host_name, "constant")?;
        validate_ref(&constant.ty, &type_ids)?;
    }

    reject_cycles(&all_types)?;
    Ok(())
}

fn validate_resource_lifecycle_name(
    rust_name: &str,
    host_name: &str,
    resource: &str,
) -> Result<()> {
    if [rust_name, host_name]
        .into_iter()
        .any(|name| matches!(name, "close" | "free"))
    {
        bail!("resource `{resource}` reserves lifecycle names `close` and `free`");
    }
    Ok(())
}

const fn target_includes_python(target: Target) -> bool {
    matches!(target, Target::Both | Target::Python)
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

    let kind = field_kind(&field.ty, type_definitions, &mut BTreeSet::new());
    if (field.constraints.min_length.is_some() || field.constraints.max_length.is_some())
        && !matches!(kind, FieldKind::String | FieldKind::List)
    {
        bail!("field `{label}` applies length constraints to a non-string/list type");
    }
    if field.constraints.ge.is_some() && !matches!(kind, FieldKind::Integer { .. }) {
        bail!("field `{label}` applies ge to a non-integer type");
    }
    for (name, value) in [
        ("literal", field.constraints.literal.as_ref()),
        ("default", field.default.as_ref()),
    ] {
        if let Some(value) = value {
            validate_scalar(value, kind, &label, name)?;
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
        (ScalarValue::I64(_), FieldKind::Json) => true,
        _ => false,
    };
    if !valid {
        bail!("field `{field}` has a {name} incompatible with its type");
    }
    Ok(())
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
