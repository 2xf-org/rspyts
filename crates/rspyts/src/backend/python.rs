//! Python conversions using native Python values and a private buffer payload.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{
    PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple,
};

use crate::wire::{BufferDtype, BufferValue, WireValue};
use crate::{ir::TypeDef, ir::TypeRef};

use super::{BackendError, decode_buffer_bytes, encode_buffer_bytes, normalize_wire};

/// Private native payload used between generated Python and PyO3 wrappers.
///
/// A dedicated class makes numeric buffers unambiguous: an ordinary mapping or
/// JSON object can never be mistaken for a buffer because it happens to contain
/// a reserved marker key.
#[pyclass(frozen, name = "BufferPayload")]
pub struct BufferPayload {
    buffer: BufferValue,
}

#[pymethods]
impl BufferPayload {
    #[new]
    fn new(dtype: &str, data: &Bound<'_, PyBytes>) -> PyResult<Self> {
        let dtype = parse_dtype(dtype).map_err(PyErr::from)?;
        let width = dtype.bytes();
        let bytes = data.as_bytes();
        if !bytes.len().is_multiple_of(width) {
            return Err(PyValueError::new_err(format!(
                "{} bytes is not a whole number of {} values",
                bytes.len(),
                dtype.name()
            )));
        }
        let buffer = decode_buffer_bytes(dtype, bytes.len() / width, bytes).map_err(PyErr::from)?;
        Ok(Self { buffer })
    }

    #[getter]
    fn dtype(&self) -> &'static str {
        self.buffer.dtype().name()
    }

    #[getter]
    fn length(&self) -> usize {
        self.buffer.len()
    }

    #[getter]
    fn little_endian(&self) -> bool {
        true
    }

    #[getter]
    fn data(&self, py: Python<'_>) -> Py<PyBytes> {
        PyBytes::new(py, &encode_buffer_bytes(&self.buffer)).unbind()
    }
}

/// Adds the private buffer payload type to the generated native module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<BufferPayload>()
}

impl From<BackendError> for PyErr {
    fn from(error: BackendError) -> Self {
        PyValueError::new_err(error.to_string())
    }
}

/// Converts a wire value to an owned Python object.
pub fn encode(py: Python<'_>, value: &WireValue) -> PyResult<Py<PyAny>> {
    value.validate().map_err(BackendError::from_boundary)?;
    encode_at(py, value)
}

fn encode_at(py: Python<'_>, value: &WireValue) -> PyResult<Py<PyAny>> {
    match value {
        WireValue::Null => Ok(py.None()),
        WireValue::Bool(value) => Ok(PyBool::new(py, *value).to_owned().into_any().unbind()),
        WireValue::I64(value) => Ok(PyInt::new(py, *value).into_any().unbind()),
        WireValue::U64(value) => Ok(PyInt::new(py, *value).into_any().unbind()),
        WireValue::F64(value) => Ok(PyFloat::new(py, *value).into_any().unbind()),
        WireValue::String(value) => Ok(PyString::new(py, value).into_any().unbind()),
        WireValue::Sequence(values) => {
            let list = PyList::empty(py);
            for value in values {
                list.append(encode_at(py, value)?)?;
            }
            Ok(list.into_any().unbind())
        }
        WireValue::Object(values) => {
            let dict = PyDict::new(py);
            for (key, value) in values {
                dict.set_item(key, encode_at(py, value)?)?;
            }
            Ok(dict.into_any().unbind())
        }
        WireValue::Bytes(bytes) => Ok(PyBytes::new(py, bytes).into_any().unbind()),
        WireValue::Buffer(buffer) => encode_buffer(py, buffer),
    }
}

/// Converts a Python value to the complete wire vocabulary.
///
/// Numeric buffers must arrive in the private payload emitted by
/// [`encode_buffer`]. Generated Python wrappers construct that payload from
/// NumPy arrays, memoryviews, or other accepted Python buffer providers.
pub fn decode(value: &Bound<'_, PyAny>) -> Result<WireValue, BackendError> {
    decode_at(value, "$".to_owned())
}

/// Converts a wire value to Python after recursive contract validation.
///
/// Generated wrappers should use this entrypoint so nested integer ranges,
/// buffer dtypes, strict fields, and enum variants are checked consistently.
pub fn encode_typed(
    py: Python<'_>,
    value: &WireValue,
    ty: &TypeRef,
    types: &[TypeDef],
) -> PyResult<Py<PyAny>> {
    let value = normalize_wire(value, ty, types).map_err(PyErr::from)?;
    encode_at(py, &value)
}

/// Converts Python to wire data with recursive contract validation.
pub fn decode_typed(
    value: &Bound<'_, PyAny>,
    ty: &TypeRef,
    types: &[TypeDef],
) -> Result<WireValue, BackendError> {
    normalize_wire(&decode(value)?, ty, types)
}

/// Decodes a Python integer against an exact signed 64-bit contract field.
pub fn decode_i64(value: &Bound<'_, PyAny>) -> Result<i64, BackendError> {
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(BackendError::UnsupportedValue {
            host: "Python",
            path: "$".to_owned(),
            actual: "expected int for i64".to_owned(),
        });
    }
    value
        .extract::<i64>()
        .map_err(|_| BackendError::IntegerOutOfRange {
            host: "Python",
            expected: "i64",
        })
}

/// Decodes a Python integer against an exact unsigned 64-bit contract field.
pub fn decode_u64(value: &Bound<'_, PyAny>) -> Result<u64, BackendError> {
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(BackendError::UnsupportedValue {
            host: "Python",
            path: "$".to_owned(),
            actual: "expected int for u64".to_owned(),
        });
    }
    value
        .extract::<u64>()
        .map_err(|_| BackendError::IntegerOutOfRange {
            host: "Python",
            expected: "u64",
        })
}

/// Decodes Python `bytes` or `bytearray` to owned Rust bytes.
pub fn decode_bytes(value: &Bound<'_, PyAny>) -> Result<Vec<u8>, BackendError> {
    if let Ok(bytes) = value.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(bytes) = value.cast::<PyByteArray>() {
        return Ok(bytes.to_vec());
    }
    Err(BackendError::UnsupportedValue {
        host: "Python",
        path: "$".to_owned(),
        actual: "expected bytes or bytearray".to_owned(),
    })
}

fn decode_at(value: &Bound<'_, PyAny>, path: String) -> Result<WireValue, BackendError> {
    if value.is_none() {
        return Ok(WireValue::Null);
    }
    if value.is_instance_of::<PyBool>() {
        return value
            .extract::<bool>()
            .map(WireValue::Bool)
            .map_err(|error| object_error("Python", path, error));
    }
    if value.is_instance_of::<PyInt>() {
        if let Ok(integer) = value.extract::<i64>() {
            return Ok(WireValue::I64(integer));
        }
        return value.extract::<u64>().map(WireValue::U64).map_err(|_| {
            BackendError::IntegerOutOfRange {
                host: "Python",
                expected: "i64 or u64",
            }
        });
    }
    if value.is_instance_of::<PyFloat>() {
        let number = value
            .extract::<f64>()
            .map_err(|error| object_error("Python", path, error))?;
        if !number.is_finite() {
            return Err(BackendError::NonFiniteStructuredFloat);
        }
        return Ok(WireValue::F64(number));
    }
    if value.is_instance_of::<PyString>() {
        return value
            .extract::<String>()
            .map(WireValue::String)
            .map_err(|error| object_error("Python", path, error));
    }
    if let Ok(bytes) = value.cast::<PyBytes>() {
        return Ok(WireValue::Bytes(bytes.as_bytes().to_vec()));
    }
    if let Ok(bytes) = value.cast::<PyByteArray>() {
        return Ok(WireValue::Bytes(bytes.to_vec()));
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        return decode_dict(dict, path).map(WireValue::Object);
    }
    if value.extract::<PyRef<'_, BufferPayload>>().is_ok() {
        return decode_buffer(value).map(WireValue::Buffer);
    }
    if let Ok(list) = value.cast::<PyList>() {
        let mut decoded = Vec::with_capacity(list.len());
        for (index, item) in list.iter().enumerate() {
            decoded.push(decode_at(&item, format!("{path}[{index}]"))?);
        }
        return Ok(WireValue::Sequence(decoded));
    }
    if let Ok(tuple) = value.cast::<PyTuple>() {
        let mut decoded = Vec::with_capacity(tuple.len());
        for (index, item) in tuple.iter().enumerate() {
            decoded.push(decode_at(&item, format!("{path}[{index}]"))?);
        }
        return Ok(WireValue::Sequence(decoded));
    }

    let actual = value
        .get_type()
        .name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    Err(BackendError::UnsupportedValue {
        host: "Python",
        path,
        actual,
    })
}

fn decode_dict(
    dict: &Bound<'_, PyDict>,
    path: String,
) -> Result<BTreeMap<String, WireValue>, BackendError> {
    let mut values = BTreeMap::new();
    for (key, value) in dict.iter() {
        let key = key
            .extract::<String>()
            .map_err(|_| BackendError::UnsupportedValue {
                host: "Python",
                path: path.clone(),
                actual: "mapping with a non-string key".to_owned(),
            })?;
        values.insert(key.clone(), decode_at(&value, format!("{path}.{key}"))?);
    }
    Ok(values)
}

/// Emits the private payload whose data property returns owned `bytes`.
///
/// Generated Python performs `numpy.frombuffer(data, dtype=...).reshape(...)`
/// (and may copy when a mutable result is required). No NumPy C/Rust dependency
/// is needed in the extension module.
pub fn encode_buffer(py: Python<'_>, buffer: &BufferValue) -> PyResult<Py<PyAny>> {
    Py::new(
        py,
        BufferPayload {
            buffer: buffer.clone(),
        },
    )
    .map(Py::into_any)
}

/// Decodes the stable private numeric-buffer payload.
pub fn decode_buffer(payload: &Bound<'_, PyAny>) -> Result<BufferValue, BackendError> {
    payload
        .extract::<PyRef<'_, BufferPayload>>()
        .map(|payload| payload.buffer.clone())
        .map_err(|_| BackendError::InvalidBuffer("expected native BufferPayload".to_owned()))
}

fn parse_dtype(name: &str) -> Result<BufferDtype, BackendError> {
    match name {
        "u8" => Ok(BufferDtype::U8),
        "i8" => Ok(BufferDtype::I8),
        "u16" => Ok(BufferDtype::U16),
        "i16" => Ok(BufferDtype::I16),
        "u32" => Ok(BufferDtype::U32),
        "i32" => Ok(BufferDtype::I32),
        "u64" => Ok(BufferDtype::U64),
        "i64" => Ok(BufferDtype::I64),
        "f32" => Ok(BufferDtype::F32),
        "f64" => Ok(BufferDtype::F64),
        other => Err(BackendError::InvalidBuffer(format!(
            "unsupported dtype {other:?}"
        ))),
    }
}

fn object_error(host: &'static str, path: impl Into<String>, error: PyErr) -> BackendError {
    BackendError::ObjectAccess {
        host,
        path: path.into(),
        message: error.to_string(),
    }
}
