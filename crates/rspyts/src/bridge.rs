//! Direct Serde adapters for the Python and WebAssembly boundaries.

#[cfg(not(target_arch = "wasm32"))]
pub mod python {
    use pyo3::exceptions::{PyRuntimeError, PyValueError};
    use pyo3::prelude::*;
    use serde::{Serialize, de::DeserializeOwned};

    /// Deserialize a Python value through Serde.
    ///
    /// # Errors
    ///
    /// Returns `ValueError` when the Python value does not match `T`.
    pub fn from_host<T: DeserializeOwned>(value: &Bound<'_, PyAny>) -> PyResult<T> {
        serde_pyobject::from_pyobject(value.clone())
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Serialize a Rust value into Python through Serde.
    ///
    /// # Errors
    ///
    /// Returns `ValueError` when Serde cannot represent the value in Python.
    pub fn to_host<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
        serde_pyobject::to_pyobject(py, value)
            .map(Bound::unbind)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Convert a typed contract error into the native Python exception payload.
    pub fn contract_error(error: &impl crate::runtime::ContractError) -> PyErr {
        PyRuntimeError::new_err((error.code(), error.to_string()))
    }
}

#[cfg(target_arch = "wasm32")]
pub mod wasm {
    use serde::{Serialize, de::DeserializeOwned};
    use wasm_bindgen::JsValue;

    /// Deserialize a JavaScript value through Serde.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript string when the value does not match `T`.
    pub fn from_host<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
        serde_wasm_bindgen::from_value(value).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Serialize a Rust value into JavaScript through Serde.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript string when Serde cannot represent the value.
    pub fn to_host<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
        let serializer = serde_wasm_bindgen::Serializer::new()
            .serialize_missing_as_null(true)
            .serialize_maps_as_objects(true)
            .serialize_large_number_types_as_bigints(true);
        value
            .serialize(&serializer)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Convert a typed contract error into the native JavaScript error payload.
    pub fn contract_error(error: &impl crate::runtime::ContractError) -> JsValue {
        JsValue::from_str(&format!("{}\n{}", error.code(), error))
    }
}
