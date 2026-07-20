use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;

use crate::ir::TypeRef;

/// Describe how a Rust type appears in the generated host APIs.
pub trait ContractType {
    /// Return the host-neutral reference for this Rust type.
    fn type_ref() -> TypeRef;
}

macro_rules! integer {
    ($type:ty, $signed:literal, $bits:literal) => {
        impl ContractType for $type {
            fn type_ref() -> TypeRef {
                TypeRef::Int {
                    signed: $signed,
                    bits: $bits,
                }
            }
        }
    };
}

integer!(u8, false, 8);
integer!(u16, false, 16);
integer!(u32, false, 32);
integer!(u64, false, 64);
integer!(i8, true, 8);
integer!(i16, true, 16);
integer!(i32, true, 32);
integer!(i64, true, 64);

impl ContractType for () {
    fn type_ref() -> TypeRef {
        TypeRef::Unit
    }
}

impl ContractType for bool {
    fn type_ref() -> TypeRef {
        TypeRef::Bool
    }
}

impl ContractType for f32 {
    fn type_ref() -> TypeRef {
        TypeRef::Float { bits: 32 }
    }
}

impl ContractType for f64 {
    fn type_ref() -> TypeRef {
        TypeRef::Float { bits: 64 }
    }
}

impl ContractType for String {
    fn type_ref() -> TypeRef {
        TypeRef::String
    }
}

impl ContractType for str {
    fn type_ref() -> TypeRef {
        TypeRef::String
    }
}

impl ContractType for chrono::DateTime<chrono::Utc> {
    fn type_ref() -> TypeRef {
        TypeRef::DateTime
    }
}

impl ContractType for chrono::DateTime<chrono::FixedOffset> {
    fn type_ref() -> TypeRef {
        TypeRef::DateTime
    }
}

impl ContractType for serde_json::Value {
    fn type_ref() -> TypeRef {
        TypeRef::Json
    }
}

impl<T: ContractType> ContractType for Option<T> {
    fn type_ref() -> TypeRef {
        TypeRef::Option {
            item: Box::new(T::type_ref()),
        }
    }
}

impl<T: ContractType> ContractType for Vec<T> {
    fn type_ref() -> TypeRef {
        TypeRef::List {
            item: Box::new(T::type_ref()),
        }
    }
}

impl<T: ContractType> ContractType for [T] {
    fn type_ref() -> TypeRef {
        TypeRef::List {
            item: Box::new(T::type_ref()),
        }
    }
}

impl<T: ContractType> ContractType for BTreeMap<String, T> {
    fn type_ref() -> TypeRef {
        TypeRef::Map {
            value: Box::new(T::type_ref()),
        }
    }
}

impl<T: ContractType, S: BuildHasher> ContractType for HashMap<String, T, S> {
    fn type_ref() -> TypeRef {
        TypeRef::Map {
            value: Box::new(T::type_ref()),
        }
    }
}

impl<T: ContractType + ?Sized> ContractType for &T {
    fn type_ref() -> TypeRef {
        T::type_ref()
    }
}

macro_rules! tuple {
    ($($name:ident),+) => {
        impl<$($name: ContractType),+> ContractType for ($($name,)+) {
            fn type_ref() -> TypeRef {
                TypeRef::Tuple {
                    items: vec![$($name::type_ref()),+],
                }
            }
        }
    };
}

tuple!(A, B);
tuple!(A, B, C);
tuple!(A, B, C, D);
tuple!(A, B, C, D, E);
tuple!(A, B, C, D, E, F);
tuple!(A, B, C, D, E, F, G);
tuple!(A, B, C, D, E, F, G, H);
