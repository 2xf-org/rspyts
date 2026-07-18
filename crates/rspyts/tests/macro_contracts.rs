use rspyts::ir::{BufferElement, DefinitionId, ScalarValue, Target, TypeRef};

#[derive(Debug, thiserror::Error, rspyts::Error)]
#[error("alias failure")]
pub struct AliasError;

type Result<T> = std::result::Result<T, AliasError>;
const DIGEST_LENGTH: usize = 8;

#[rspyts::export]
#[rspyts(returns(buffer), error = AliasError)]
pub fn alias_values() -> Result<Vec<f64>> {
    Ok(vec![1.0])
}

pub struct Counter(u64);

#[derive(rspyts::Type)]
pub struct Record {
    #[rspyts(literal = 2)]
    pub revision: u16,
    #[rspyts(min_length = 1, max_length = 200)]
    pub items: Vec<String>,
    #[rspyts(default = 1, ge = 1, le = 200)]
    pub count: u64,
    #[rspyts(default = "unknown")]
    pub category: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(rspyts::Type)]
pub struct FixedRecord {
    #[rspyts(bytes)]
    pub digest: [u8; DIGEST_LENGTH],
    pub history: Vec<DigestValue>,
    pub optional: Option<DigestValue>,
}

#[derive(rspyts::Type)]
#[serde(transparent)]
pub struct DigestValue(#[rspyts(bytes)] pub [u8; DIGEST_LENGTH]);

#[rspyts::export]
#[rspyts(returns(bytes))]
pub fn echo_digest(#[rspyts(bytes)] value: &[u8; DIGEST_LENGTH]) -> [u8; DIGEST_LENGTH] {
    *value
}

#[rspyts::export(python)]
pub const PYTHON_ONLY: u16 = 2;

/// Shared signed revision marker.
#[rspyts::export]
pub const BOTH_TARGETS: i16 = -2;

#[rspyts::export(typescript)]
pub const TYPESCRIPT_ONLY: &str = "typescript";

/// Static-only feature marker.
#[rspyts::export(static)]
pub static STATIC_ONLY: bool = true;

mod python_surface {
    #[rspyts::export(python)]
    pub fn emitter_scoped_name() -> u16 {
        1
    }

    #[rspyts::export(python)]
    pub const EMITTER_SCOPED_VALUE: u16 = 1;
}

mod typescript_surface {
    #[rspyts::export(typescript)]
    pub fn emitter_scoped_name() -> u16 {
        2
    }

    #[rspyts::export(typescript)]
    pub const EMITTER_SCOPED_VALUE: u16 = 2;
}

/// A persistent counter resource.
#[rspyts::export]
impl Counter {
    #[rspyts(constructor, error = AliasError)]
    pub fn open() -> Result<Self> {
        Ok(Self(1))
    }

    #[rspyts(error = AliasError)]
    pub fn value(&self) -> Result<u64> {
        Ok(self.0)
    }
}

#[test]
fn one_parameter_result_aliases_have_explicit_error_identity() {
    let manifest =
        rspyts::registry::manifest("rspyts", env!("CARGO_PKG_VERSION"), "native").unwrap();
    let function = manifest
        .functions
        .iter()
        .find(|function| function.host_name == "aliasValues")
        .unwrap();
    assert_eq!(
        function.returns,
        TypeRef::Buffer {
            element: BufferElement::F64,
        }
    );
    assert_eq!(
        function.error,
        Some(DefinitionId::new("rspyts", "macro_contracts::AliasError"))
    );

    let resource = manifest
        .resources
        .iter()
        .find(|resource| resource.name == "Counter")
        .unwrap();
    assert_eq!(resource.constructors[0].error, function.error);
    assert_eq!(resource.methods[0].error, function.error);
}

#[test]
fn field_semantics_and_constant_targets_are_registered_exactly() {
    let manifest =
        rspyts::registry::manifest("rspyts", env!("CARGO_PKG_VERSION"), "native").unwrap();
    let record = manifest
        .types
        .iter()
        .find(|definition| definition.name == "Record")
        .unwrap();
    let rspyts::ir::TypeShape::Struct { fields } = &record.shape else {
        panic!("Record should be a struct");
    };
    assert_eq!(fields[0].constraints.literal, Some(ScalarValue::I64(2)));
    assert_eq!(fields[1].constraints.min_length, Some(1));
    assert_eq!(fields[1].constraints.max_length, Some(200));
    assert_eq!(fields[2].default, Some(ScalarValue::I64(1)));
    assert_eq!(fields[2].constraints.ge, Some(1));
    assert_eq!(fields[2].constraints.le, Some(200));
    assert!(!fields[2].required);
    assert_eq!(
        fields[3].default,
        Some(ScalarValue::String("unknown".to_owned()))
    );
    assert_eq!(fields[4].ty, TypeRef::DateTime);

    let target = |name: &str| {
        manifest
            .constants
            .iter()
            .find(|constant| constant.host_name == name)
            .unwrap()
            .target
    };
    assert_eq!(target("PYTHON_ONLY"), Target::Python);
    assert_eq!(target("BOTH_TARGETS"), Target::Both);
    assert_eq!(target("TYPESCRIPT_ONLY"), Target::Typescript);
    assert_eq!(target("STATIC_ONLY"), Target::Static);

    let constant_docs = |name: &str| {
        manifest
            .constants
            .iter()
            .find(|constant| constant.host_name == name)
            .and_then(|constant| constant.docs.as_deref())
    };
    assert_eq!(
        constant_docs("BOTH_TARGETS"),
        Some("Shared signed revision marker.")
    );
    assert_eq!(
        constant_docs("STATIC_ONLY"),
        Some("Static-only feature marker.")
    );

    let counter = manifest
        .resources
        .iter()
        .find(|resource| resource.name == "Counter")
        .unwrap();
    assert_eq!(
        counter.docs.as_deref(),
        Some("A persistent counter resource.")
    );
}

#[test]
fn registry_allows_macro_exports_to_reuse_names_on_disjoint_host_surfaces() {
    assert_eq!(python_surface::emitter_scoped_name(), 1);
    assert_eq!(typescript_surface::emitter_scoped_name(), 2);
    let manifest =
        rspyts::registry::manifest("rspyts", env!("CARGO_PKG_VERSION"), "native").unwrap();
    assert_eq!(
        manifest
            .functions
            .iter()
            .filter(|item| item.host_name == "emitterScopedName")
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        [Target::Python, Target::Typescript]
    );
    assert_eq!(
        manifest
            .constants
            .iter()
            .filter(|item| item.host_name == "EMITTER_SCOPED_VALUE")
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        [Target::Python, Target::Typescript]
    );
}

#[test]
fn fixed_byte_lengths_survive_fields_aliases_containers_and_exports() {
    let manifest =
        rspyts::registry::manifest("rspyts", env!("CARGO_PKG_VERSION"), "native").unwrap();
    let fixed = manifest
        .types
        .iter()
        .find(|definition| definition.name == "FixedRecord")
        .unwrap();
    let rspyts::ir::TypeShape::Struct { fields } = &fixed.shape else {
        panic!("FixedRecord should be a struct");
    };
    assert_eq!(fields[0].ty, TypeRef::FixedBytes { length: 8 });
    assert_eq!(
        fields[1].ty,
        TypeRef::List {
            item: Box::new(TypeRef::Named {
                identity: DefinitionId::new("rspyts", "macro_contracts::DigestValue"),
            }),
        }
    );
    assert_eq!(
        fields[2].ty,
        TypeRef::Option {
            item: Box::new(TypeRef::Named {
                identity: DefinitionId::new("rspyts", "macro_contracts::DigestValue"),
            }),
        }
    );

    let alias = manifest
        .types
        .iter()
        .find(|definition| definition.name == "DigestValue")
        .unwrap();
    assert_eq!(
        alias.shape,
        rspyts::ir::TypeShape::Alias {
            target: TypeRef::FixedBytes { length: 8 },
        }
    );

    let function = manifest
        .functions
        .iter()
        .find(|function| function.host_name == "echoDigest")
        .unwrap();
    assert_eq!(function.params[0].ty, TypeRef::FixedBytes { length: 8 });
    assert_eq!(function.returns, TypeRef::FixedBytes { length: 8 });
}
