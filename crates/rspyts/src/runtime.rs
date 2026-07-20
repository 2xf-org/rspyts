//! Runtime registration and typed error support.

use serde_json::Value;

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

    pub struct Registration(pub fn(&Bound<'_, PyModule>) -> PyResult<()>);

    inventory::collect!(Registration);

    pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
        for registration in inventory::iter::<Registration> {
            (registration.0)(module)?;
        }
        Ok(())
    }
}
