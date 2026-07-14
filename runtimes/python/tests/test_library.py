"""
Library resolution, loading, and calling tests — driven by an in-memory cdylib double.
"""

import ctypes
import dataclasses
import json
import struct
import sys
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from rspyts import BridgeError, Library, RspytsPanicError, StaleHandleError, envelope, library


@pytest.fixture(autouse=True)
def clean_env(monkeypatch):
    """
    Keep the developer's real RSPYTS_LIBRARY out of the tests.
    """
    monkeypatch.delenv("RSPYTS_LIBRARY", raising=False)


@pytest.mark.parametrize(
    ("platform", "expected"),
    [
        ("darwin", "libdemo_crate.dylib"),
        ("linux", "libdemo_crate.so"),
        ("win32", "demo_crate.dll"),
    ],
)
def test_platform_filename(monkeypatch, platform, expected):
    monkeypatch.setattr(sys, "platform", platform)
    # Hyphens in the crate name normalize to underscores.
    assert library.platform_filename("demo-crate") == expected
    assert library.platform_filename("demo_crate") == expected


def touch(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"")
    return path


def test_env_var_wins_over_everything(monkeypatch, tmp_path):
    env_path = tmp_path / "from_env.dylib"
    monkeypatch.setenv("RSPYTS_LIBRARY", str(env_path))
    lib = Library("demo", search=[str(tmp_path)])
    lib.set_path(tmp_path / "explicit.dylib")
    assert lib.resolve_path() == env_path


@pytest.mark.parametrize(
    ("use_env", "use_set_path", "winner"),
    [
        (True, True, "env"),
        (True, False, "env"),
        (False, True, "explicit"),
        (False, False, "search"),
    ],
)
def test_resolution_precedence_matrix(monkeypatch, tmp_path, use_env, use_set_path, winner):
    search_hit = touch(tmp_path / "dist" / library.platform_filename("demo"))
    env_path = tmp_path / "env.dylib"
    explicit = tmp_path / "explicit.dylib"
    if use_env:
        monkeypatch.setenv("RSPYTS_LIBRARY", str(env_path))
    lib = Library("demo", search=["dist"], anchor=tmp_path)
    if use_set_path:
        lib.set_path(explicit)
    expected = {"env": env_path, "explicit": explicit, "search": search_hit}[winner]
    assert lib.resolve_path() == expected


def test_empty_env_var_is_ignored(monkeypatch, tmp_path):
    monkeypatch.setenv("RSPYTS_LIBRARY", "")
    expected = touch(tmp_path / library.platform_filename("demo"))
    lib = Library("demo", search=[str(tmp_path)])
    assert lib.resolve_path() == expected


def test_set_path_wins_over_search(tmp_path):
    touch(tmp_path / library.platform_filename("demo"))
    explicit = tmp_path / "explicit.dylib"
    lib = Library("demo", search=[str(tmp_path)])
    lib.set_path(explicit)
    assert lib.resolve_path() == explicit


def test_set_path_after_load_is_an_error(tmp_path):
    lib = Library("demo")
    lib.cdll = object()  # simulate a completed load
    with pytest.raises(RuntimeError, match="already loaded"):
        lib.set_path(tmp_path / "late.dylib")


def test_absolute_search_dir(tmp_path):
    expected = touch(tmp_path / library.platform_filename("demo"))
    lib = Library("demo", search=[str(tmp_path)])
    assert lib.resolve_path() == expected


def test_relative_search_dir_resolves_against_anchor(tmp_path):
    expected = touch(tmp_path / "target" / "debug" / library.platform_filename("demo"))
    lib = Library("demo", search=["target/debug"], anchor=tmp_path)
    assert lib.resolve_path() == expected


def test_relative_search_dir_without_anchor_uses_cwd(tmp_path, monkeypatch):
    expected = touch(tmp_path / "dist" / library.platform_filename("demo"))
    monkeypatch.chdir(tmp_path)
    lib = Library("demo", search=["dist"])
    assert lib.resolve_path() == expected


def test_first_search_hit_wins(tmp_path):
    first = touch(tmp_path / "a" / library.platform_filename("demo"))
    touch(tmp_path / "b" / library.platform_filename("demo"))
    lib = Library("demo", search=[str(tmp_path / "a"), str(tmp_path / "b")])
    assert lib.resolve_path() == first


def test_search_skips_missing_directories(tmp_path):
    expected = touch(tmp_path / "b" / library.platform_filename("demo"))
    lib = Library("demo", search=[str(tmp_path / "a"), str(tmp_path / "b")])
    assert lib.resolve_path() == expected


def test_missing_library_error_lists_candidates(tmp_path):
    lib = Library("demo", search=["target/debug"], anchor=tmp_path)
    with pytest.raises(FileNotFoundError) as exc_info:
        lib.resolve_path()
    message = str(exc_info.value)
    assert str(tmp_path / "target" / "debug" / library.platform_filename("demo")) in message
    assert "RSPYTS_LIBRARY" in message


def test_no_search_dirs_error_is_still_helpful():
    lib = Library("demo")
    with pytest.raises(FileNotFoundError, match="no search directories"):
        lib.resolve_path()


def test_missing_library_error_lists_every_candidate(tmp_path):
    lib = Library("demo", search=[str(tmp_path / "a"), "rel/b", str(tmp_path / "c")], anchor=tmp_path)
    with pytest.raises(FileNotFoundError) as exc_info:
        lib.resolve_path()
    message = str(exc_info.value)
    filename = library.platform_filename("demo")
    for directory in (tmp_path / "a", tmp_path / "rel" / "b", tmp_path / "c"):
        assert str(directory / filename) in message


# ---------------------------------------------------------------------------
# In-memory cdylib double
# ---------------------------------------------------------------------------


class FakeExport:
    """
    Stands in for one ctypes foreign function: callable, restype assignable.
    """

    def __init__(self, impl) -> None:
        self.impl = impl
        self.restype = None
        self.argtypes = None

    def __call__(self, *args: Any) -> Any:
        return self.impl(*args)


def cvalue(arg: Any) -> Any:
    """
    Unwrap a ctypes scalar to its plain Python value.

    Args:
        arg: A ctypes instance or an already-plain value.

    Returns:
        The underlying Python int (or the value unchanged).
    """
    return arg.value if hasattr(arg, "value") else arg


@dataclasses.dataclass(frozen=True, slots=True)
class Call:
    """
    One recorded bridged call, snapshotted while its buffers were alive.
    """

    handle: int | None
    request: bytes
    slices: list[np.ndarray]


class FakeCdll:
    """
    An in-memory cdylib double implementing the mandatory ABI symbols.

    Notes:
        Allocations are real ctypes buffers, so Library.call's memmove and
        string_at operate on genuine addresses. Frees are recorded so tests
        can assert both the request and the envelope get released; recorded
        calls snapshot their argument buffers at call time, since Library
        frees the request before returning.
    """

    def __init__(self, abi_version: int = 2) -> None:
        self.live: dict[int, Any] = {}
        self.freed: list[tuple[int, int]] = []
        self.rspyts_abi_version = FakeExport(lambda: abi_version)
        self.rspyts_alloc = FakeExport(self.alloc)
        self.rspyts_free = FakeExport(self.free)

    def alloc(self, size: Any) -> int:
        n = int(cvalue(size))
        buf = ctypes.create_string_buffer(max(n, 1))
        addr = ctypes.addressof(buf)
        self.live[addr] = buf
        return addr

    def free(self, ptr: Any, size: Any) -> None:
        self.freed.append((int(cvalue(ptr)), int(cvalue(size))))
        self.live.pop(int(cvalue(ptr)), None)

    def install(
        self,
        symbol: str,
        status: int = 0,
        payload: Any = None,
        tail: bytes = b"",
        slice_dts: tuple[str, ...] = (),
    ) -> list[Call]:
        """
        Register an export that records its arguments and returns an envelope.

        Args:
            symbol: The C symbol name to expose.
            status: The envelope status byte to answer with.
            payload: The JSON payload to answer with.
            tail: The raw numeric tail to answer with.
            slice_dts: Wire dtype names of the expected slice arguments, in
                declaration order, used to snapshot their memory.

        Returns:
            The (live) list every recorded call is appended to.
        """
        calls: list[Call] = []

        def impl(*args: Any) -> int:
            handle = args[0].value if isinstance(args[0], ctypes.c_uint64) else None
            idx = 0 if handle is None else 1
            request = ctypes.string_at(cvalue(args[idx]), cvalue(args[idx + 1]))
            pairs = [(cvalue(args[i]), cvalue(args[i + 1])) for i in range(idx + 2, len(args), 2)]
            snapshots = [
                np.frombuffer(
                    ctypes.string_at(ptr, count * np.dtype(envelope.DTYPES[dt]).itemsize),
                    dtype=envelope.DTYPES[dt],
                )
                for dt, (ptr, count) in zip(slice_dts, pairs, strict=True)
            ]
            calls.append(Call(handle=handle, request=request, slices=snapshots))
            body = json.dumps(payload, separators=(",", ":")).encode()
            raw = bytes([status, 0, 0, 0]) + struct.pack("<II", len(body), len(tail)) + body + tail
            addr = self.alloc(len(raw))
            ctypes.memmove(addr, raw, len(raw))
            return addr

        setattr(self, symbol, FakeExport(impl))
        return calls


def bridged(fake: FakeCdll) -> Library:
    """
    A Library whose lazy load already resolved to the given double.

    Args:
        fake: The cdylib double to answer calls with.

    Returns:
        A Library ready for call / call_drop without touching the disk.
    """
    lib = Library("demo")
    lib.cdll = fake
    return lib


def request_payload(call: Call) -> Any:
    payload, _tail = envelope.decode_request(call.request)
    return payload


# ---------------------------------------------------------------------------
# Loading (ABI version handshake)
# ---------------------------------------------------------------------------


def test_load_checks_abi_version_and_caches(monkeypatch, tmp_path):
    fake = FakeCdll(abi_version=2)
    opened: list[str] = []

    def fake_cdll(path):
        opened.append(path)
        return fake

    monkeypatch.setattr(ctypes, "CDLL", fake_cdll)
    monkeypatch.setenv("RSPYTS_LIBRARY", str(tmp_path / "demo.dylib"))
    lib = Library("demo")

    assert lib.load() is fake
    assert lib.load() is fake  # second load hits the cache
    assert opened == [str(tmp_path / "demo.dylib")]
    assert fake.rspyts_abi_version.restype is ctypes.c_uint32
    assert fake.rspyts_alloc.restype is ctypes.c_void_p
    assert fake.rspyts_alloc.argtypes == (ctypes.c_size_t,)
    assert fake.rspyts_free.restype is None


def test_load_rejects_abi_version_mismatch(monkeypatch, tmp_path):
    monkeypatch.setattr(ctypes, "CDLL", lambda path: FakeCdll(abi_version=7))
    monkeypatch.setenv("RSPYTS_LIBRARY", str(tmp_path / "demo.dylib"))
    lib = Library("demo")
    with pytest.raises(RuntimeError, match="ABI version 7") as exc_info:
        lib.load()
    assert "version 2" in str(exc_info.value)
    assert lib.cdll is None  # a failed handshake caches nothing


# ---------------------------------------------------------------------------
# Calling (request encoding, envelope decoding, memory discipline)
# ---------------------------------------------------------------------------


def test_call_sends_compact_json_and_decodes_payload():
    fake = FakeCdll()
    calls = fake.install("demo__add", payload={"sum": 3})

    result = bridged(fake).call("demo__add", {"a": 1, "minimumValue": 1.5})

    assert result == {"sum": 3}
    assert request_payload(calls[0]) == {"a": 1, "minimumValue": 1.5}
    assert calls[0].handle is None


def test_call_sends_nested_numpy_buffers_in_the_request_envelope():
    fake = FakeCdll()
    calls = fake.install("demo__buffered", payload=None)
    values = np.array([-2, 7], dtype=np.int32)

    bridged(fake).call("demo__buffered", {"nested": [{"values": values}]})

    payload, tail = envelope.decode_request(calls[0].request)
    decoded = envelope.substitute_buffers(payload, tail)
    assert decoded["nested"][0]["values"].dtype == np.int32
    np.testing.assert_array_equal(decoded["nested"][0]["values"], values)
    assert fake.live == {}
    assert fake.freed[0][1] == len(calls[0].request)


def test_call_with_no_args_sends_empty_object():
    fake = FakeCdll()
    calls = fake.install("demo__ping", payload=None)

    assert bridged(fake).call("demo__ping", None) is None
    assert request_payload(calls[0]) == {}


def test_call_passes_handle_as_leading_u64():
    fake = FakeCdll()
    calls = fake.install("demo__method", payload=0)

    bridged(fake).call("demo__method", {}, handle=42)

    assert calls[0].handle == 42
    assert request_payload(calls[0]) == {}


def test_call_passes_slices_as_contiguous_pointer_count_pairs():
    fake = FakeCdll()
    calls = fake.install("demo__analyze", payload=None, slice_dts=("f64", "i32"))
    strided = np.arange(8, dtype=np.int64)[::2]  # non-contiguous, wrong dtype

    bridged(fake).call("demo__analyze", {}, slices=[([1.5, 2.5], "f64"), (strided, "i32")])

    first, second = calls[0].slices
    assert first.dtype == np.float64
    np.testing.assert_array_equal(first, [1.5, 2.5])
    assert second.dtype == np.int32
    np.testing.assert_array_equal(second, [0, 2, 4, 6])


def test_call_borrows_only_runtime_owned_slice_copies():
    fake = FakeCdll()
    source = np.array([1.0, 2.0], dtype=np.float64)
    seen_pointer: list[int] = []

    def inspect(*args: Any) -> int:
        seen_pointer.append(int(cvalue(args[2])))
        body = b"null"
        raw = bytes([0, 0, 0, 0]) + struct.pack("<II", len(body), 0) + body
        addr = fake.alloc(len(raw))
        ctypes.memmove(addr, raw, len(raw))
        return addr

    fake.demo__inspect = FakeExport(inspect)
    bridged(fake).call("demo__inspect", {}, slices=[(source, "f64")])

    assert seen_pointer[0] != source.ctypes.data


def test_call_substitutes_buffers_from_the_tail():
    values = np.array([1.0, 2.0, 4.0], dtype=np.float32)
    fake = FakeCdll()
    fake.install(
        "demo__spectrum",
        payload={"bins": {"__rspyts_buf__": {"off": 0, "len": 3, "dt": "f32"}}},
        tail=values.tobytes(),
    )

    result = bridged(fake).call("demo__spectrum", {})

    np.testing.assert_array_equal(result["bins"], values)


def test_call_status_1_raises_registered_error():
    fake = FakeCdll()
    fake.install("demo__get", status=1, payload={"code": "staleHandle", "message": "dropped"})
    with pytest.raises(StaleHandleError, match="dropped"):
        bridged(fake).call("demo__get", {})


def test_call_uses_scoped_error_types():
    class ScopedError(BridgeError):
        pass

    fake = FakeCdll()
    fake.install("demo__get", status=1, payload={"code": "scoped", "message": "only here"})
    with pytest.raises(ScopedError, match="only here"):
        bridged(fake).call("demo__get", {}, error_types={"scoped": ScopedError})


def test_call_status_2_raises_panic_error():
    fake = FakeCdll()
    fake.install("demo__boom", status=2, payload={"code": "panic", "message": "kaboom"})
    with pytest.raises(RspytsPanicError, match="kaboom"):
        bridged(fake).call("demo__boom", {})


def test_call_unknown_error_code_raises_bridge_error():
    fake = FakeCdll()
    fake.install("demo__fail", status=1, payload={"code": "neverRegistered", "message": "m"})
    with pytest.raises(BridgeError) as exc_info:
        bridged(fake).call("demo__fail", {})
    assert exc_info.value.code == "neverRegistered"


def test_call_null_envelope_is_a_runtime_error():
    fake = FakeCdll()
    fake.demo__null = FakeExport(lambda *args: None)
    with pytest.raises(RuntimeError, match="null envelope"):
        bridged(fake).call("demo__null", {})


def test_call_frees_request_and_envelope():
    fake = FakeCdll()
    fake.install("demo__add", payload=7, tail=b"\x01\x02")

    bridged(fake).call("demo__add", {"a": 1})

    assert fake.live == {}  # nothing leaked
    request_free, envelope_free = fake.freed
    assert request_free[1] == len(envelope.build_request({"a": 1}))
    assert envelope_free[1] == envelope.HEADER_LEN + len(b"7") + 2


def test_call_frees_request_even_when_the_export_raises():
    fake = FakeCdll()

    def explode(*args: Any) -> int:
        raise OSError("segv, politely")

    fake.demo__bad = FakeExport(explode)
    with pytest.raises(OSError, match="politely"):
        bridged(fake).call("demo__bad", {})
    assert fake.live == {}


def test_call_frees_response_when_copying_it_raises(monkeypatch):
    fake = FakeCdll()
    fake.install("demo__large", payload={"ok": True})
    real_string_at = ctypes.string_at
    calls = 0

    def fail_second_copy(ptr, size):
        nonlocal calls
        calls += 1
        if calls == 2:  # request snapshot, then the full response
            raise MemoryError("copy failed")
        return real_string_at(ptr, size)

    monkeypatch.setattr(ctypes, "string_at", fail_second_copy)
    with pytest.raises(MemoryError, match="copy failed"):
        bridged(fake).call("demo__large", {})
    assert fake.live == {}


@pytest.mark.parametrize("bad", [float("nan"), float("inf"), float("-inf")])
def test_call_rejects_non_finite_floats_in_json_position(bad):
    lib = bridged(FakeCdll())
    with pytest.raises(ValueError, match="non-finite floats") as exc_info:
        lib.call("demo__add", {"x": bad})
    assert "slice or Buf" in str(exc_info.value)


def test_call_drop_fires_without_an_envelope():
    fake = FakeCdll()
    calls: list[tuple[Any, ...]] = []
    fake.demo__drop = FakeExport(lambda *args: calls.append(args))

    assert bridged(fake).call_drop("demo__drop", 9) is None

    assert fake.demo__drop.restype is None
    assert isinstance(calls[0][0], ctypes.c_uint64)
    assert calls[0][0].value == 9
