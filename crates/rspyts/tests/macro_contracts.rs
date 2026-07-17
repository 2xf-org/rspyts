use rspyts::ir::{BufferElement, DefinitionId, ScalarValue, Target, TypeRef};

#[derive(Debug, thiserror::Error, rspyts::Error)]
#[error("alias failure")]
pub struct AliasError;

type Result<T> = std::result::Result<T, AliasError>;

#[rspyts::export]
#[rspyts(returns(buffer), error = AliasError)]
pub fn alias_samples() -> Result<Vec<f64>> {
    Ok(vec![1.0])
}

pub struct Counter(u64);

#[derive(rspyts::Type)]
pub struct Policy {
    #[rspyts(literal = 2)]
    pub contract_version: u16,
    #[rspyts(min_length = 1, max_length = 200)]
    pub batch: Vec<String>,
    #[serde(default)]
    #[rspyts(default = 1, ge = 1)]
    pub quantity: u64,
    #[serde(default)]
    #[rspyts(default = "unknown")]
    pub actor_kind: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

#[rspyts::export(python)]
pub const PYTHON_ONLY: u16 = 2;

#[rspyts::export]
pub const BOTH_TARGETS: i16 = -2;

#[rspyts::export(typescript)]
pub const TYPESCRIPT_ONLY: &str = "typescript";

#[rspyts::export(static)]
pub const STATIC_ONLY: bool = true;

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
    let manifest = rspyts::registry::manifest("rspyts", "0.4.0", "native").unwrap();
    let function = manifest
        .functions
        .iter()
        .find(|function| function.host_name == "aliasSamples")
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
    let manifest = rspyts::registry::manifest("rspyts", "0.4.0", "native").unwrap();
    let policy = manifest
        .types
        .iter()
        .find(|definition| definition.name == "Policy")
        .unwrap();
    let rspyts::ir::TypeShape::Struct { fields } = &policy.shape else {
        panic!("Policy should be a struct");
    };
    assert_eq!(fields[0].constraints.literal, Some(ScalarValue::I64(2)));
    assert_eq!(fields[1].constraints.min_length, Some(1));
    assert_eq!(fields[1].constraints.max_length, Some(200));
    assert_eq!(fields[2].default, Some(ScalarValue::I64(1)));
    assert_eq!(fields[2].constraints.ge, Some(1));
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
}
