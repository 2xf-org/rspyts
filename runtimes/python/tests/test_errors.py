"""
Error raising and code-registry tests (ABI §5).
"""

import pytest

from rspyts import BridgeError, RspytsPanicError, StaleHandleError, errors, register_error


@pytest.fixture(autouse=True)
def isolated_registry(monkeypatch):
    """
    Give every test its own copy of the code->class registry.
    """
    monkeypatch.setattr(errors, "REGISTRY", dict(errors.REGISTRY))


def test_bridge_error_attributes_and_str():
    err = BridgeError("bad input", code="invalidArgs", data={"field": "x"})
    assert err.code == "invalidArgs"
    assert err.message == "bad input"
    assert err.data == {"field": "x"}
    assert str(err) == "[invalidArgs] bad input"


def test_data_defaults_to_none():
    assert BridgeError("m", code="c").data is None


def test_status_2_raises_panic_error():
    with pytest.raises(RspytsPanicError) as exc_info:
        errors.raise_bridge_error(2, {"code": "panic", "message": "kaboom 7"})
    assert exc_info.value.code == "panic"
    assert exc_info.value.message == "kaboom 7"
    assert exc_info.value.data is None


def test_status_1_unknown_code_falls_back_to_bridge_error():
    with pytest.raises(BridgeError) as exc_info:
        errors.raise_bridge_error(1, {"code": "neverRegistered", "message": "m"})
    assert type(exc_info.value) is BridgeError
    assert exc_info.value.code == "neverRegistered"


def test_stale_handle_is_preregistered():
    with pytest.raises(StaleHandleError):
        errors.raise_bridge_error(1, {"code": "staleHandle", "message": "dropped"})


def test_registered_class_takes_precedence():
    class BatchTooLargeError(BridgeError):
        pass

    register_error("batchTooLarge", BatchTooLargeError)
    with pytest.raises(BatchTooLargeError) as exc_info:
        errors.raise_bridge_error(1, {"code": "batchTooLarge", "message": "m", "data": {"max": 5}})
    assert exc_info.value.data == {"max": 5}


def test_call_scoped_registry_prevents_cross_package_collisions():
    class FirstPackageError(BridgeError):
        pass

    class SecondPackageError(BridgeError):
        pass

    payload = {"code": "sharedCode", "message": "scoped"}
    with pytest.raises(FirstPackageError):
        errors.raise_bridge_error(1, payload, {"sharedCode": FirstPackageError})
    with pytest.raises(SecondPackageError):
        errors.raise_bridge_error(1, payload, {"sharedCode": SecondPackageError})


def test_later_registration_wins():
    class FirstError(BridgeError):
        pass

    class SecondError(BridgeError):
        pass

    register_error("dup", FirstError)
    register_error("dup", SecondError)
    with pytest.raises(SecondError):
        errors.raise_bridge_error(1, {"code": "dup", "message": "m"})


def test_status_2_wins_over_registry():
    # A panic envelope always raises RspytsPanicError, even if some class
    # is registered for the payload's code.
    class HijackError(BridgeError):
        pass

    register_error("panic", HijackError)
    with pytest.raises(RspytsPanicError):
        errors.raise_bridge_error(2, {"code": "panic", "message": "m"})


def test_register_error_rejects_non_bridge_error_classes():
    with pytest.raises(TypeError):
        register_error("bad", ValueError)  # type: ignore[arg-type]


def test_panic_and_stale_are_bridge_errors():
    # Callers can catch the whole family with one except clause.
    assert issubclass(RspytsPanicError, BridgeError)
    assert issubclass(StaleHandleError, BridgeError)
    assert StaleHandleError("m").code == "staleHandle"
    assert RspytsPanicError("m").code == "panic"


def test_stale_handle_isinstance_chain():
    err = StaleHandleError("dropped")
    assert isinstance(err, StaleHandleError)
    assert isinstance(err, BridgeError)
    assert isinstance(err, Exception)


@pytest.mark.parametrize(
    ("err", "expected"),
    [
        (StaleHandleError("handle 3 was dropped"), "[staleHandle] handle 3 was dropped"),
        (RspytsPanicError("index out of bounds"), "[panic] index out of bounds"),
    ],
)
def test_subclass_str_uses_code_message_format(err, expected):
    assert str(err) == expected
