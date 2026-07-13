"""
End-to-end tests: Python → ctypes → Rust cdylib → envelope → pydantic.

Notes:
    Requires the native library: ``cargo build -p basic-example`` (the
    generated ``library.py`` searches ``target/{debug,release}``; override
    with the ``RSPYTS_LIBRARY`` env var, which CI sets explicitly).
"""

import concurrent.futures
import json
import re
import threading
from pathlib import Path

import numpy as np
import pydantic
import pytest
import rspyts

import basic_example as bx

SCHEMA_PATH = Path(__file__).parents[2] / "schema" / "schema.json"


class TestSummarize:
    def test_computes_stats(self):
        summary = bx.summarize(np.array([2.0, 4.0, 6.0]), "demo")
        assert isinstance(summary, bx.Summary)
        assert summary.item_count == 3
        assert summary.total == 12.0
        assert summary.average == 4.0
        assert summary.label == "demo"

    def test_optional_label_may_be_none(self):
        assert bx.summarize(np.array([1.0]), None).label is None

    def test_accepts_plain_lists(self):
        # Library converts anything array-like via np.ascontiguousarray.
        assert bx.summarize([1.0, 3.0], None).average == 2.0

    def test_empty_input_raises_typed_error(self):
        with pytest.raises(bx.BasicErrorEmptyInput) as exc_info:
            bx.summarize(np.array([]), None)
        assert exc_info.value.code == "emptyInput"

    def test_typed_errors_are_catchable_as_base(self):
        with pytest.raises(bx.BasicError):
            bx.summarize(np.array([]), None)
        with pytest.raises(rspyts.BridgeError):
            bx.summarize(np.array([]), None)

    def test_rejects_more_than_max_window_values(self):
        with pytest.raises(bx.BasicErrorTooManyValues) as exc_info:
            bx.summarize(np.ones(bx.MAX_WINDOW + 1), None)
        assert exc_info.value.data == {"count": bx.MAX_WINDOW + 1}
        assert bx.summarize(np.ones(bx.MAX_WINDOW), None).item_count == bx.MAX_WINDOW


class TestScale:
    def test_returns_numpy_buffer(self):
        out = bx.scale(np.array([1.0, 2.0, 3.0]), 2.0)
        assert isinstance(out, np.ndarray)
        assert out.dtype == np.float64
        np.testing.assert_allclose(out, [2.0, 4.0, 6.0])

    def test_large_buffer_round_trip(self):
        values = np.linspace(0.0, 1.0, 100_000)
        out = bx.scale(values, 3.0)
        assert out.shape == values.shape
        np.testing.assert_allclose(out, values * 3.0)

    def test_zero_factor_carries_data(self):
        with pytest.raises(bx.BasicErrorZeroFactor) as exc_info:
            bx.scale(np.array([1.0]), 0.0)
        assert exc_info.value.data == {"factor": 0.0}

    def test_non_finite_scalars_fail_fast_client_side(self):
        # JSON has no Infinity/NaN; the bridge rejects them before the call
        # (raw slice and Buf payloads carry non-finite values just fine).
        with pytest.raises(ValueError, match="non-finite"):
            bx.scale(np.array([1.0]), float("inf"))


class TestRoundValue:
    def test_string_enum_parameter(self):
        assert bx.round_value(2.4, bx.Rounding.UP) == 3.0
        assert bx.round_value(2.6, bx.Rounding.DOWN) == 2.0
        assert bx.round_value(2.5, bx.Rounding.NEAREST) == 3.0

    def test_half_even_breaks_ties_toward_even(self):
        # Variant wire values are camelCase — the convention IS the contract.
        assert bx.Rounding.HALF_EVEN.value == "halfEven"
        assert bx.round_value(2.5, bx.Rounding.HALF_EVEN) == 2.0
        assert bx.round_value(3.5, bx.Rounding.HALF_EVEN) == 4.0


class TestConstants:
    def test_scalar_constant_is_importable_and_correct(self):
        assert bx.MAX_WINDOW == 1024
        assert isinstance(bx.MAX_WINDOW, int)

    def test_list_constant_matches_the_enum_wire_values(self):
        assert bx.ROUNDING_MODES == ["up", "down", "nearest", "halfEven"]
        assert bx.ROUNDING_MODES == [mode.value for mode in bx.Rounding]


class TestAnnotate:
    def test_json_dict_round_trips_untouched(self):
        metadata = {"source": "sensor", "nested": {"tags": ["a", "b"], "rev": 2}, "empty": None}
        out = bx.annotate(2.5, metadata)
        assert out == {**metadata, "value": 2.5}

    def test_non_object_metadata_is_wrapped(self):
        assert bx.annotate(1.0, [1, "two", False]) == {"input": [1, "two", False], "value": 1.0}
        assert bx.annotate(0.5, "note") == {"input": "note", "value": 0.5}


class TestLoadValues:
    def test_reads_one_number_per_line(self, tmp_path):
        path = tmp_path / "values.txt"
        path.write_text("1.5\n\n  2\n-3.25\n")
        assert bx.load_values(str(path)) == [1.5, 2.0, -3.25]

    def test_missing_file_raises_typed_error(self, tmp_path):
        missing = tmp_path / "nope.txt"
        with pytest.raises(bx.BasicErrorUnreadableFile) as exc_info:
            bx.load_values(str(missing))
        assert exc_info.value.data == {"path": str(missing)}

    def test_python_only_scoping_leaves_the_typescript_client_clean(self):
        # `#[bridge(target = "python")]`: present here, absent from the
        # generated TypeScript client next door.
        client_src = Path(__file__).parents[2] / "typescript" / "src" / "generated" / "client.ts"
        assert callable(bx.load_values)
        assert "loadValues" not in client_src.read_text()


class TestParseNumber:
    def test_discriminated_union_round_trip(self):
        integer = bx.parse_number("42")
        assert isinstance(integer, bx.ParsedNumberInteger)
        assert integer.type == "integer"
        assert integer.value == 42

        decimal = bx.parse_number("3.5")
        assert isinstance(decimal, bx.ParsedNumberDecimal)
        assert decimal.value == 3.5

    def test_not_a_number_carries_text(self):
        with pytest.raises(bx.BasicErrorNotANumber) as exc_info:
            bx.parse_number("abc")
        assert exc_info.value.data == {"text": "abc"}


class TestCounter:
    def test_factory_returns_a_live_handle(self):
        counter = bx.Counter.starting_at_zero("fresh")
        assert isinstance(counter, bx.Counter)
        assert counter.current_value() == 0
        assert counter.increment(3) == 3
        assert counter.label() == "fresh"
        counter.close()
        with pytest.raises(rspyts.StaleHandleError):
            counter.current_value()

    def test_factory_and_constructor_instances_are_independent(self):
        with bx.Counter(10, "ctor") as constructed, bx.Counter.starting_at_zero("factory") as made:
            constructed.increment(1)
            made.increment(2)
            assert constructed.current_value() == 11
            assert made.current_value() == 2

    def test_static_helper_needs_no_instance(self):
        assert bx.Counter.default_label() == "unnamed"

    def test_lifecycle(self):
        counter = bx.Counter(10, "bench")
        assert counter.increment(5) == 15
        assert counter.add_parsed("7") == 22
        assert counter.current_value() == 22
        assert counter.label() == "bench"
        counter.reset()
        assert counter.current_value() == 10
        counter.close()

    def test_method_errors_are_typed(self):
        counter = bx.Counter(0, "err")
        with pytest.raises(bx.BasicErrorNotANumber):
            counter.add_parsed("1.5")
        assert counter.current_value() == 0
        counter.close()

    def test_context_manager_and_stale_handle(self):
        with bx.Counter(0, "ctx") as counter:
            counter.increment(1)
        with pytest.raises(rspyts.StaleHandleError):
            counter.increment(1)

    def test_close_is_idempotent(self):
        counter = bx.Counter(0, "idem")
        counter.close()
        counter.close()

    def test_instances_are_independent(self):
        a = bx.Counter(0, "a")
        b = bx.Counter(100, "b")
        a.increment(1)
        b.increment(1)
        assert a.current_value() == 1
        assert b.current_value() == 101
        a.close()
        b.close()


class TestPanics:
    def test_panic_becomes_typed_exception(self):
        with pytest.raises(rspyts.RspytsPanicError, match="intentional panic"):
            bx.simulate_panic()

    def test_library_survives_a_panic(self):
        with pytest.raises(rspyts.RspytsPanicError):
            bx.simulate_panic()
        assert bx.summarize(np.array([1.0, 2.0]), None).item_count == 2


class TestContractValidation:
    def test_camel_case_aliases(self):
        summary = bx.Summary.model_validate({"itemCount": 3, "total": 12.0, "average": 4.0, "label": None})
        assert summary.item_count == 3

    def test_unknown_fields_rejected(self):
        with pytest.raises(Exception):
            bx.Summary.model_validate({"itemCount": 3, "total": 12.0, "average": 4.0, "label": None, "bogus": 1})


class TestUnicode:
    def test_labels_round_trip_exactly(self):
        for label in ["📈🚀", "统计概要", "ملخص عربي", "mix 数🚀 عرب"]:
            assert bx.summarize(np.array([1.0]), label).label == label

    def test_parse_error_preserves_unicode_text(self):
        garbage = "١٢٣ ≠ 数字 🚀"
        with pytest.raises(bx.BasicErrorNotANumber) as exc_info:
            bx.parse_number(garbage)
        assert exc_info.value.data == {"text": garbage}


class TestBoundaries:
    def test_i32_extremes_through_counter(self):
        with bx.Counter(2_147_483_647, "max") as counter:
            assert counter.current_value() == 2_147_483_647
        with bx.Counter(0, "walk") as counter:
            assert counter.increment(2_147_483_647) == 2_147_483_647
            counter.reset()
            assert counter.increment(-2_147_483_648) == -2_147_483_648

    def test_item_count_rejects_out_of_range_u32(self):
        for bad in (-1, 2**32):
            with pytest.raises(pydantic.ValidationError):
                bx.Summary.model_validate({"itemCount": bad, "total": 0.0, "average": 0.0, "label": None})

    def test_empty_string_label_is_not_none(self):
        assert bx.summarize(np.array([1.0]), "").label == ""
        assert bx.summarize(np.array([1.0]), None).label is None


class TestBulkBuffers:
    def test_empty_input_scales_to_empty_output(self):
        out = bx.scale(np.array([], dtype=np.float64), 2.0)
        assert isinstance(out, np.ndarray)
        assert out.dtype == np.float64
        assert out.size == 0

    def test_million_element_round_trip(self):
        values = np.arange(1_000_000, dtype=np.float64)
        out = bx.scale(values, 2.0)
        assert out.shape == values.shape
        assert out[0] == 0.0
        assert out[123_456] == 246_912.0
        assert out[999_999] == 1_999_998.0

    def test_f64_specials_cross_through_buffers(self):
        # JSON positions reject NaN/Infinity, but slice and Buf payloads
        # are raw bytes — specials cross intact and scale correctly.
        out = bx.scale(np.array([np.nan, np.inf, -np.inf, 0.0]), 2.0)
        assert np.isnan(out[0])
        assert out[1] == np.inf
        assert out[2] == -np.inf
        assert out[3] == 0.0


class TestHandleLifecycle:
    def test_200_simultaneous_counters_with_interleaved_ops(self):
        counters = [bx.Counter(i, f"c{i}") for i in range(200)]
        for counter in counters:
            counter.increment(1_000)
        for counter in counters[::2]:
            counter.reset()
        for i, counter in enumerate(counters):
            assert counter.current_value() == (i if i % 2 == 0 else i + 1_000)
            assert counter.label() == f"c{i}"
            counter.close()

    def test_every_method_raises_stale_after_close(self):
        counter = bx.Counter(0, "stale")
        counter.close()
        with pytest.raises(rspyts.StaleHandleError):
            counter.increment(1)
        with pytest.raises(rspyts.StaleHandleError):
            counter.add_parsed("1")
        with pytest.raises(rspyts.StaleHandleError):
            counter.current_value()
        with pytest.raises(rspyts.StaleHandleError):
            counter.label()
        with pytest.raises(rspyts.StaleHandleError):
            counter.reset()


class TestSchema:
    def test_schema_declares_every_bridged_model(self):
        schema = json.loads(SCHEMA_PATH.read_text())
        assert {"Summary", "ParsedNumber", "Rounding"} <= set(schema["$defs"])

    def test_manifest_hash_matches_generated_code(self):
        schema = json.loads(SCHEMA_PATH.read_text())
        models_src = Path(bx.generated.models.__file__).read_text()
        match = re.search(r"# rspyts:manifest-hash (\S+)", models_src)
        assert match is not None
        assert schema["x-rspyts"]["manifestHash"] == match.group(1)


class TestConcurrency:
    def test_eight_threads_call_simultaneously(self):
        # ctypes releases the GIL for the duration of each C call, so all
        # eight workers hit the library at once; results must stay exact.
        barrier = threading.Barrier(8)

        def worker(seed: int) -> None:
            values = np.full(1_000, float(seed + 1))
            barrier.wait()
            for _ in range(50):
                summary = bx.summarize(values, f"t{seed}")
                assert summary.item_count == 1_000
                assert summary.average == seed + 1.0
                assert bx.scale(values, 3.0)[999] == (seed + 1) * 3.0

        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            for future in [pool.submit(worker, seed) for seed in range(8)]:
                future.result()
