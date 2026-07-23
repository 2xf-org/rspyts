//! Value conversion at the Python and WebAssembly boundaries.
//!
//! Ordinary contract values use Serde-shaped conversion. Annotated byte and
//! numeric-buffer boundaries use specialized host representations so large
//! contiguous values avoid object-by-object serialization.

/// Python conversion primitives used by generated native wrappers.
#[cfg(not(target_arch = "wasm32"))]
pub mod python {
    use std::{mem, slice};

    use pyo3::buffer::{Element, PyBuffer};
    use pyo3::exceptions::PyBufferError;
    use pyo3::exceptions::{PyRuntimeError, PyValueError};
    use pyo3::prelude::*;
    use pyo3::types::PyBytes;
    use serde::{Serialize, de::DeserializeOwned};

    /// Deserialize a Python value through Serde.
    ///
    /// # Errors
    ///
    /// Returns `ValueError` when the Python value does not match `T`.
    pub fn from_host<T: DeserializeOwned>(value: &Bound<'_, PyAny>) -> PyResult<T> {
        pythonize::depythonize(value).map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Copy a Python bytes object into Rust-owned memory.
    ///
    /// # Errors
    ///
    /// Returns `BufferError` when `value` is not a one-dimensional,
    /// C-contiguous byte buffer.
    pub fn bytes_from_host(value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
        if let Ok(bytes) = value.cast::<PyBytes>() {
            return Ok(bytes.as_bytes().to_vec());
        }
        buffer_from_host(value)
    }

    /// Copy a typed Python buffer into Rust-owned memory.
    ///
    /// # Errors
    ///
    /// Returns `BufferError` unless `value` exposes a one-dimensional,
    /// C-contiguous, native-endian buffer with the exact element type `T`.
    pub fn buffer_from_host<T: Element>(value: &Bound<'_, PyAny>) -> PyResult<Vec<T>> {
        let buffer = PyBuffer::<T>::get(value)?;
        if buffer.dimensions() != 1 {
            return Err(PyBufferError::new_err(
                "rspyts buffers must be one-dimensional",
            ));
        }
        if !buffer.is_c_contiguous() {
            return Err(PyBufferError::new_err(
                "rspyts buffers must be C-contiguous",
            ));
        }
        buffer.to_vec(value.py())
    }

    /// Return bytes through Python's native immutable byte representation.
    pub fn bytes_to_host(py: Python<'_>, value: &[u8]) -> Py<PyAny> {
        PyBytes::new(py, value).into_any().unbind()
    }

    mod sealed {
        pub trait Sealed {}
    }

    /// A primitive element supported by an rspyts numeric buffer.
    #[doc(hidden)]
    pub trait PythonBufferElement: sealed::Sealed + Copy {}

    macro_rules! buffer_element {
        ($($ty:ty),+ $(,)?) => {
            $(
                impl sealed::Sealed for $ty {}
                impl PythonBufferElement for $ty {}
            )+
        };
    }

    buffer_element!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

    /// Return a numeric buffer as native-endian raw bytes.
    ///
    /// Generated Python restores this backing store with `numpy.frombuffer`.
    pub fn buffer_to_host<T: PythonBufferElement>(py: Python<'_>, value: &[T]) -> Py<PyAny> {
        let length = mem::size_of_val(value);
        // SAFETY: every `PythonBufferElement` is a plain numeric primitive,
        // and the resulting byte slice cannot outlive `value` during this call.
        let bytes = unsafe { slice::from_raw_parts(value.as_ptr().cast::<u8>(), length) };
        bytes_to_host(py, bytes)
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

/// JavaScript conversion primitives used by generated WebAssembly wrappers.
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
