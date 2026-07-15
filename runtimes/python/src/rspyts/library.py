"""Load and call an ABI 3 bridged native library."""

from __future__ import annotations

import collections.abc
import ctypes
import os
import pathlib
import re
import sys
import threading
import typing

import numpy as np

from . import envelope, errors

__all__ = ["Library"]

ENV_VAR_PREFIX = "RSPYTS_LIBRARY"
EXPECTED_ABI_VERSION = 3
MAX_HANDLE = 2**53 - 1
FINGERPRINT_PATTERN = re.compile(r"[0-9a-f]{64}\Z")


def library_env_var(name: str) -> str:
    """Return the library-specific native override variable for ``name``."""
    suffix = "".join(character.upper() if character.isascii() and character.isalnum() else "_" for character in name)
    return f"{ENV_VAR_PREFIX}_{suffix}"


def platform_filename(name: str) -> str:
    """Return the platform-specific shared-library filename for a crate name."""
    stem = name.replace("-", "_")
    if sys.platform == "darwin":
        return f"lib{stem}.dylib"
    if sys.platform == "win32":
        return f"{stem}.dll"
    return f"lib{stem}.so"


def _checked_fingerprint(value: object, *, source: str) -> str:
    if type(value) is not str or FINGERPRINT_PATTERN.fullmatch(value) is None:
        raise RuntimeError(f"rspyts: {source} must be a lowercase 64-character SHA-256 fingerprint")
    return value


def _checked_handle(handle: object) -> int:
    if type(handle) is not int:
        raise TypeError(f"rspyts: handle must be an exact integer, got {type(handle).__name__}")
    if not 1 <= handle <= MAX_HANDLE:
        raise ValueError(f"rspyts: handle must be in 1..={MAX_HANDLE}, got {handle}")
    return handle


class Library:
    """A lazily loaded ABI 3 cdylib with one schema-directed call path."""

    def __init__(
        self,
        name: str,
        search: collections.abc.Sequence[str] = (),
        anchor: str | pathlib.Path | None = None,
        *,
        expected_contract_fingerprint: str | None = None,
    ) -> None:
        self.name = name
        self.search = tuple(search)
        self.anchor = pathlib.Path(anchor) if anchor is not None else None
        self.expected_contract_fingerprint = (
            _checked_fingerprint(expected_contract_fingerprint, source="expected contract fingerprint")
            if expected_contract_fingerprint is not None
            else None
        )
        self.contract_fingerprint: str | None = None
        self.explicit_path: pathlib.Path | None = None
        self.cdll: ctypes.CDLL | None = None
        self.load_lock = threading.Lock()

    def set_path(self, path: str | pathlib.Path) -> None:
        """Override library resolution before the first load."""
        if self.cdll is not None:
            raise RuntimeError(
                f"rspyts: library {self.name!r} is already loaded; "
                "set_path must be called before the first bridged call"
            )
        self.explicit_path = pathlib.Path(path)

    def resolve_path(self) -> pathlib.Path:
        """Resolve the on-disk native library path."""
        env_var = library_env_var(self.name)
        env = os.environ.get(env_var)
        if env:
            return pathlib.Path(env)
        if self.explicit_path is not None:
            return self.explicit_path
        filename = platform_filename(self.name)
        candidates: list[pathlib.Path] = []
        for entry in self.search:
            directory = pathlib.Path(entry)
            if not directory.is_absolute():
                directory = (self.anchor or pathlib.Path.cwd()) / directory
            candidate = directory / filename
            candidates.append(candidate)
            if candidate.is_file():
                return candidate
        raise FileNotFoundError(
            f"rspyts: cannot locate the compiled library for {self.name!r}. "
            f"Tried: {', '.join(str(candidate) for candidate in candidates) or '(no search directories)'}. "
            f"Build the crate, or point {env_var} at the library file."
        )

    def load(self) -> ctypes.CDLL:
        """Load, ABI-check, fingerprint-check, and cache the cdylib."""
        cdll = self.cdll
        if cdll is not None:
            return cdll
        with self.load_lock:
            if self.cdll is not None:
                return self.cdll
            path = self.resolve_path()
            cdll = ctypes.CDLL(str(path))
            try:
                cdll.rspyts_abi_version.restype = ctypes.c_uint32
                cdll.rspyts_abi_version.argtypes = ()
                version = cdll.rspyts_abi_version()
            except AttributeError as exc:
                raise RuntimeError(f"rspyts: {path} does not export rspyts_abi_version") from exc
            if version != EXPECTED_ABI_VERSION:
                raise RuntimeError(
                    f"rspyts: {path} reports ABI version {version}, but this runtime speaks version "
                    f"{EXPECTED_ABI_VERSION}. Rebuild the crate and regenerate the bindings, or install "
                    "the matching rspyts package version."
                )

            try:
                cdll.rspyts_alloc.restype = ctypes.c_void_p
                cdll.rspyts_alloc.argtypes = (ctypes.c_size_t,)
                cdll.rspyts_free.restype = None
                cdll.rspyts_free.argtypes = (ctypes.c_void_p, ctypes.c_size_t)
                cdll.rspyts_contract_fingerprint.restype = ctypes.c_void_p
                cdll.rspyts_contract_fingerprint.argtypes = ()
            except AttributeError as exc:
                raise RuntimeError(f"rspyts: {path} is missing a required ABI 3 export: {exc}") from exc

            ret = cdll.rspyts_contract_fingerprint()
            if ret is None:
                raise RuntimeError("rspyts: rspyts_contract_fingerprint returned a null envelope")
            raw = self._copy_and_free_response(cdll, int(ret))
            status, response = envelope.parse_envelope(raw)
            if status != 0:
                raise RuntimeError(f"rspyts: rspyts_contract_fingerprint failed with status {status}")
            if response.tail:
                raise RuntimeError("rspyts: rspyts_contract_fingerprint returned attachment bytes")
            fingerprint = _checked_fingerprint(response.value, source="module contract fingerprint")
            expected = self.expected_contract_fingerprint
            if expected is not None and fingerprint != expected:
                raise RuntimeError(
                    f"rspyts: contract fingerprint mismatch for {path}: module reports {fingerprint}, "
                    f"generated client expects {expected}; rebuild the crate and regenerate the bindings"
                )

            self.contract_fingerprint = fingerprint
            self.cdll = cdll
            return cdll

    def call(
        self,
        symbol: str,
        args_obj: collections.abc.Mapping[str, typing.Any] | None,
        slices: collections.abc.Sequence[tuple[typing.Any, str]] = (),
        handle: int | None = None,
        error_types: errors.BridgeErrorRegistry | None = None,
    ) -> envelope.Response:
        """Invoke one bridge and return its explicit value-plus-tail response."""
        cdll = self.load()
        encoded = envelope.build_request(args_obj)

        arrays: list[np.ndarray] = []
        for data, dt in slices:
            dtype = envelope.DTYPES.get(dt)
            if dtype is None:
                raise ValueError(f"rspyts: unsupported slice dtype {dt!r}")
            arrays.append(np.require(data, dtype=dtype, requirements=("C", "A", "O")).copy(order="C"))

        try:
            fn = getattr(cdll, symbol)
        except AttributeError as exc:
            raise RuntimeError(f"rspyts: module has no export {symbol!r}") from exc
        fn.restype = ctypes.c_void_p

        c_args: list[typing.Any] = []
        if handle is not None:
            c_args.append(ctypes.c_uint64(_checked_handle(handle)))

        request_len = len(encoded)
        request_ptr = cdll.rspyts_alloc(request_len)
        if request_ptr is None:
            raise MemoryError(f"rspyts: rspyts_alloc returned null for a {request_len}-byte request")
        ctypes.memmove(request_ptr, encoded, request_len)
        c_args.append(ctypes.c_void_p(request_ptr))
        c_args.append(ctypes.c_size_t(request_len))

        for array in arrays:
            c_args.append(ctypes.c_void_p(array.ctypes.data))
            c_args.append(ctypes.c_size_t(array.size))

        try:
            ret = fn(*c_args)
        finally:
            cdll.rspyts_free(ctypes.c_void_p(request_ptr), ctypes.c_size_t(request_len))
        if ret is None:
            raise RuntimeError(f"rspyts: {symbol} returned a null envelope")

        raw = self._copy_and_free_response(cdll, int(ret))
        status, response = envelope.parse_envelope(raw)
        if status == 0:
            return response
        errors.raise_bridge_error(status, response.value, error_types)

    def call_drop(self, symbol: str, handle: int) -> None:
        """Invoke an idempotent ``__drop`` export, which returns no envelope."""
        cdll = self.load()
        try:
            fn = getattr(cdll, symbol)
        except AttributeError as exc:
            raise RuntimeError(f"rspyts: module has no export {symbol!r}") from exc
        fn.restype = None
        fn(ctypes.c_uint64(_checked_handle(handle)))

    @staticmethod
    def _copy_and_free_response(cdll: ctypes.CDLL, ret: int) -> bytes:
        """Copy one owned native response and free it with its exact header-derived length."""
        header = (ctypes.c_ubyte * envelope.HEADER_LEN).from_address(ret)
        json_len = sum(int(header[4 + index]) << (8 * index) for index in range(4))
        tail_len = sum(int(header[8 + index]) << (8 * index) for index in range(4))
        total = envelope.HEADER_LEN + json_len + tail_len
        try:
            return ctypes.string_at(ret, total)
        finally:
            cdll.rspyts_free(ctypes.c_void_p(ret), ctypes.c_size_t(total))
