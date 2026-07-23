//! Runtime registration and typed error support.
//!
//! Generated wrappers submit native registrations through `inventory`, while
//! [`ContractError`] supplies the stable identity and code carried to hosts.

/// A Rust error with a stable code for generated host exceptions.
pub trait ContractError: std::fmt::Display {
    /// Return the identity registered by the error derive.
    fn type_identity() -> crate::ir::DefinitionId
    where
        Self: Sized;

    /// Return the stable code for this error value.
    fn code(&self) -> String;
}

/// Registration support for the generated Python extension module.
#[cfg(not(target_arch = "wasm32"))]
pub mod python {
    use pyo3::prelude::*;

    /// A function that adds one generated export to a Python module.
    pub struct Registration(
        /// Register the generated export on the supplied extension module.
        pub fn(&Bound<'_, PyModule>) -> PyResult<()>,
    );

    inventory::collect!(Registration);

    /// Add every linked Python export to an extension module.
    ///
    /// # Errors
    ///
    /// Returns a Python exception when an export cannot be added to the module.
    pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
        for registration in inventory::iter::<Registration> {
            (registration.0)(module)?;
        }
        Ok(())
    }
}
