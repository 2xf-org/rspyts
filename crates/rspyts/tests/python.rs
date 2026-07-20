#![cfg(not(target_arch = "wasm32"))]

mod fixtures;

use std::sync::Once;

use rspyts::__private::pyo3::{
    Bound, PyErr, PyResult, Python,
    types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyModule},
};

fn with_registered_module<T>(
    run: impl for<'py> FnOnce(Python<'py>, &Bound<'py, PyModule>) -> PyResult<T>,
) -> PyResult<T> {
    static INITIALIZE: Once = Once::new();
    INITIALIZE.call_once(Python::initialize);
    Python::attach(|py| {
        let module = PyModule::new(py, "native")?;
        rspyts::runtime::python::register(&module)?;
        run(py, &module)
    })
}

fn job_request(py: Python<'_>, count: u32) -> PyResult<Bound<'_, PyDict>> {
    let request = PyDict::new(py);
    request.set_item("displayName", "release")?;
    request.set_item("count", count)?;
    request.set_item("mode", "deterministic")?;
    request.set_item("payload", PyList::new(py, [1_u8, 2, 3])?)?;
    request.set_item("samples", vec![1.5_f64, 2.0, 4.5])?;
    request.set_item("dryRun", false)?;
    Ok(request)
}

fn error_details(py: Python<'_>, error: &PyErr) -> PyResult<(String, String)> {
    error.value(py).getattr("args")?.extract()
}

#[test]
fn functions_decode_call_and_encode_real_values() -> PyResult<()> {
    with_registered_module(|py, module| {
        assert!(module.hasattr("executeJob")?);
        assert!(module.hasattr("reverseBytes")?);
        assert!(module.hasattr("scaleSamples")?);

        let result = module
            .getattr("executeJob")?
            .call1((job_request(py, 3)?,))?;
        let result = result.cast::<PyDict>()?;
        assert_eq!(
            result
                .get_item("id")?
                .expect("result must contain id")
                .extract::<String>()?,
            "job-release"
        );
        assert_eq!(
            result
                .get_item("acceptedCount")?
                .expect("result must contain acceptedCount")
                .extract::<u32>()?,
            3
        );
        let sample_total = result
            .get_item("sampleTotal")?
            .expect("result must contain sampleTotal")
            .extract::<f64>()?;
        assert!((sample_total - 8.0).abs() < f64::EPSILON);
        let event = result
            .get_item("event")?
            .expect("result must contain event")
            .cast_into::<PyDict>()?;
        assert_eq!(
            event
                .get_item("kind")?
                .expect("event must contain kind")
                .extract::<String>()?,
            "completed"
        );

        let reversed = module
            .getattr("reverseBytes")?
            .call1((PyList::new(py, [3_u8, 1, 4])?,))?
            .extract::<Vec<u8>>()?;
        assert_eq!(reversed, [4, 1, 3]);

        let scaled = module
            .getattr("scaleSamples")?
            .call1((vec![1.5_f64, -2.0], 2.0))?
            .extract::<Vec<f64>>()?;
        assert_eq!(scaled, [3.0, -4.0]);

        Ok(())
    })
}

#[test]
fn errors_keep_the_stable_code_and_message() -> PyResult<()> {
    with_registered_module(|py, module| {
        let error = module
            .getattr("executeJob")?
            .call1((job_request(py, 0)?,))
            .expect_err("an invalid request must fail");
        assert_eq!(
            error_details(py, &error)?,
            (
                "invalid_count".to_owned(),
                "count must be between 1 and 100".to_owned(),
            )
        );

        Ok(())
    })
}

#[test]
fn resources_support_factories_state_and_close() -> PyResult<()> {
    with_registered_module(|py, module| {
        let counter_type = module.getattr("Counter")?;

        let negative = counter_type
            .call1((-1,))
            .expect_err("a negative counter must fail");
        assert_eq!(
            error_details(py, &negative)?,
            (
                "negative_start".to_owned(),
                "the initial counter value cannot be negative".to_owned(),
            )
        );

        let counter = counter_type.call1((10,))?;
        assert_eq!(counter.call_method1("add", (5,))?.extract::<i64>()?, 15);
        assert!(!counter.hasattr("reset")?);
        counter.call_method0("close")?;
        let closed = counter
            .call_method1("add", (1,))
            .expect_err("a closed resource must reject later calls");
        assert_eq!(closed.to_string(), "RuntimeError: resource is closed");

        let zero = counter_type.call_method0("fromZero")?;
        assert_eq!(zero.call_method1("add", (2,))?.extract::<i64>()?, 2);

        Ok(())
    })
}
