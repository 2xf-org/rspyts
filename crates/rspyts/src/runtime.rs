use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallError {
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum CallOutcome<T: Serialize> {
    Ok { value: T },
    Error { error: CallError },
}

impl<T: Serialize> CallOutcome<T> {
    pub fn success(value: T) -> Self {
        Self::Ok { value }
    }

    pub fn failure(error: impl ContractError) -> Self {
        Self::Error {
            error: CallError {
                code: error.code(),
                message: error.to_string(),
                data: error.data(),
            },
        }
    }
}

pub trait ContractError: std::fmt::Display {
    fn type_identity() -> crate::ir::DefinitionId
    where
        Self: Sized;

    fn code(&self) -> String;

    fn data(&self) -> Option<Value> {
        None
    }
}

#[cfg(all(feature = "python", not(target_arch = "wasm32")))]
pub mod python {
    use pyo3::prelude::*;

    pub struct Registration {
        pub owner: &'static str,
        pub register: fn(&Bound<'_, PyModule>) -> PyResult<()>,
    }

    inventory::collect!(Registration);

    pub fn register(owner: &str, module: &Bound<'_, PyModule>) -> PyResult<()> {
        crate::backend::python::register(module)?;
        for registration in inventory::iter::<Registration> {
            if registration.owner == owner {
                (registration.register)(module)?;
            }
        }
        Ok(())
    }
}
