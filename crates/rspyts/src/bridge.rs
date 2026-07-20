//! Direct Serde adapters for the Python and WebAssembly boundaries.

#[cfg(all(feature = "python", not(target_arch = "wasm32")))]
pub mod python {
    use pyo3::exceptions::{PyRuntimeError, PyValueError};
    use pyo3::prelude::*;
    use serde::{Serialize, de::DeserializeOwned};

    pub fn from_host<T: DeserializeOwned>(value: &Bound<'_, PyAny>) -> PyResult<T> {
        serde_pyobject::from_pyobject(value.clone())
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    pub fn to_host<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
        serde_pyobject::to_pyobject(py, value)
            .map(Bound::unbind)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    pub fn contract_error(error: &impl crate::runtime::ContractError) -> PyErr {
        PyRuntimeError::new_err((error.code(), error.to_string()))
    }
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm {
    use serde::{Serialize, de::DeserializeOwned};
    use wasm_bindgen::JsValue;

    pub fn from_host<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
        serde_wasm_bindgen::from_value(value).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn to_host<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
        let serializer = serde_wasm_bindgen::Serializer::new()
            .serialize_missing_as_null(true)
            .serialize_maps_as_objects(true)
            .serialize_large_number_types_as_bigints(true);
        value
            .serialize(&serializer)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn contract_error(error: &impl crate::runtime::ContractError) -> JsValue {
        JsValue::from_str(&format!("{}\n{}", error.code(), error))
    }
}
