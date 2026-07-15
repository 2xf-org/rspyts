"""Strict bridge-error decoding and call-scoped dispatch tests."""

import pytest

from rspyts import BridgeError, RspytsPanicError, StaleHandleError, errors


def test_bridge_error_attributes_and_string():
    error = BridgeError("bad input", code="invalidArgs", data={"field": "x"})
    assert error.code == "invalidArgs"
    assert error.message == "bad input"
    assert error.data == {"field": "x"}
    assert str(error) == "[invalidArgs] bad input"
    assert BridgeError("message", code="code").data is None


def test_panic_payload_raises_the_fixed_panic_type():
    with pytest.raises(RspytsPanicError) as exc_info:
        errors.raise_bridge_error(2, {"code": "panic", "message": "kaboom", "data": {"step": 7}})
    assert exc_info.value.code == "panic"
    assert exc_info.value.data == {"step": 7}


def test_stale_handle_is_direct_without_a_global_registry():
    with pytest.raises(StaleHandleError, match="dropped"):
        errors.raise_bridge_error(1, {"code": "staleHandle", "message": "dropped"})


def test_unknown_code_falls_back_to_bridge_error():
    with pytest.raises(BridgeError) as exc_info:
        errors.raise_bridge_error(1, {"code": "neverRegistered", "message": "message"})
    assert type(exc_info.value) is BridgeError
    assert exc_info.value.code == "neverRegistered"


def test_call_scoped_registry_is_the_only_custom_dispatch_surface():
    class FirstPackageError(BridgeError):
        pass

    class SecondPackageError(BridgeError):
        pass

    payload = {"code": "sharedCode", "message": "scoped"}
    with pytest.raises(FirstPackageError):
        errors.raise_bridge_error(1, payload, {"sharedCode": FirstPackageError})
    with pytest.raises(SecondPackageError):
        errors.raise_bridge_error(1, payload, {"sharedCode": SecondPackageError})


def test_registry_entry_must_be_a_bridge_error_subclass():
    with pytest.raises(TypeError, match="not a BridgeError subclass"):
        errors.raise_bridge_error(
            1,
            {"code": "bad", "message": "message"},
            {"bad": ValueError},  # ty: ignore[invalid-argument-type]
        )


@pytest.mark.parametrize(
    ("status", "payload", "match"),
    [
        (0, {"code": "x", "message": "y"}, "response status 0"),
        (1, [], "must be a JSON object"),
        (1, {"message": "y"}, "requires exact"),
        (1, {"code": "x"}, "requires exact"),
        (1, {"code": "x", "message": "y", "extra": 1}, "unexpected fields"),
        (1, {"code": "", "message": "y"}, "non-empty JSON string"),
        (1, {"code": 1, "message": "y"}, "non-empty JSON string"),
        (1, {"code": "x", "message": 1}, "message.*JSON string"),
        (2, {"code": "notPanic", "message": "y"}, "must use code 'panic'"),
    ],
)
def test_malformed_error_envelopes_are_rejected(status, payload, match):
    with pytest.raises(ValueError, match=match):
        errors.raise_bridge_error(status, payload)


def test_public_error_inheritance_and_defaults():
    assert issubclass(RspytsPanicError, BridgeError)
    assert issubclass(StaleHandleError, BridgeError)
    assert StaleHandleError("message").code == "staleHandle"
    assert RspytsPanicError("message").code == "panic"
