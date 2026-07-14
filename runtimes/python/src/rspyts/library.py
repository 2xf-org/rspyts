"""
Load and call a bridged native library.
"""

from __future__ import annotations

import collections.abc
import ctypes
import os
import pathlib
import sys
import threading
import typing

import numpy as np

from . import envelope, errors

__all__ = ["Library", "platform_filename"]

ENV_VAR = "RSPYTS_LIBRARY"
EXPECTED_ABI_VERSION = 2


def platform_filename(name: str) -> str:
    """
    The platform-specific shared-library filename for a crate name.

    Notes:
        Hyphens are normalized to underscores (Cargo does the same when
        naming cdylib artifacts): ``basic-example`` ->
        ``libbasic_example.dylib`` / ``libbasic_example.so`` /
        ``basic_example.dll``.

    Args:
        name: The crate name.

    Returns:
        The filename the current platform's dynamic loader expects.
    """
    stem = name.replace("-", "_")
    if sys.platform == "darwin":
        return f"lib{stem}.dylib"
    if sys.platform == "win32":
        return f"{stem}.dll"
    return f"lib{stem}.so"


class Library:
    """
    A lazily-loaded bridged cdylib and the calling convention around it.

    Notes:
        Resolution order for the library path (first hit wins):

        1. the ``RSPYTS_LIBRARY`` environment variable (a full file path);
        2. an explicit :meth:`set_path` override;
        3. each ``search`` directory — absolute entries as-is, relative
           entries joined onto ``anchor`` — combined with
           :func:`platform_filename`.
    """

    def __init__(
        self,
        name: str,
        search: collections.abc.Sequence[str] = (),
        anchor: str | pathlib.Path | None = None,
    ) -> None:
        self.name = name
        self.search = tuple(search)
        self.anchor = pathlib.Path(anchor) if anchor is not None else None
        self.explicit_path: pathlib.Path | None = None
        self.cdll: ctypes.CDLL | None = None
        # Guards lazy loading only. Calls themselves need no Python-side
        # lock: ctypes releases the GIL for the duration of the C call and
        # the Rust side is internally synchronized (ABI §10).
        self.load_lock = threading.Lock()

    def set_path(self, path: str | pathlib.Path) -> None:
        """
        Override library resolution with an explicit file path.

        Notes:
            Takes effect on the next (first) load; overriding after the
            library has been loaded is a programming error.

        Args:
            path: The full path to the compiled library file.

        Raises:
            RuntimeError: If the library has already been loaded.
        """
        if self.cdll is not None:
            raise RuntimeError(
                f"rspyts: library {self.name!r} is already loaded; "
                "set_path must be called before the first bridged call"
            )
        self.explicit_path = pathlib.Path(path)

    def resolve_path(self) -> pathlib.Path:
        """
        Resolve the on-disk path of the compiled library.

        Returns:
            The first existing candidate per the resolution order.

        Raises:
            FileNotFoundError: If no candidate exists on disk.
        """
        env = os.environ.get(ENV_VAR)
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
            f"Tried: {', '.join(str(c) for c in candidates) or '(no search directories)'}. "
            f"Build the crate (cargo build), or point the {ENV_VAR} environment "
            "variable at the library file."
        )

    def load(self) -> ctypes.CDLL:
        """
        Load the cdylib once, check its ABI version, and cache it.

        Returns:
            The loaded ``ctypes.CDLL`` with the allocator symbols typed.

        Raises:
            RuntimeError: If the library reports an unexpected ABI version.
            FileNotFoundError: If the library cannot be located.
        """
        cdll = self.cdll
        if cdll is not None:
            return cdll
        with self.load_lock:
            if self.cdll is None:
                path = self.resolve_path()
                cdll = ctypes.CDLL(str(path))
                cdll.rspyts_abi_version.restype = ctypes.c_uint32
                cdll.rspyts_abi_version.argtypes = ()
                version = cdll.rspyts_abi_version()
                if version != EXPECTED_ABI_VERSION:
                    raise RuntimeError(
                        f"rspyts: {path} reports ABI version {version}, but this "
                        f"runtime speaks version {EXPECTED_ABI_VERSION}. Rebuild the "
                        "crate and regenerate the bindings (rspyts generate), or "
                        "install the matching rspyts package version."
                    )
                cdll.rspyts_alloc.restype = ctypes.c_void_p
                cdll.rspyts_alloc.argtypes = (ctypes.c_size_t,)
                cdll.rspyts_free.restype = None
                cdll.rspyts_free.argtypes = (ctypes.c_void_p, ctypes.c_size_t)
                self.cdll = cdll
            return self.cdll

    def call(
        self,
        symbol: str,
        args_obj: collections.abc.Mapping[str, typing.Any] | None,
        slices: collections.abc.Sequence[tuple[typing.Any, str]] = (),
        handle: int | None = None,
        error_types: errors.BridgeErrorRegistry | None = None,
    ) -> typing.Any:
        """
        Invoke a bridged function and decode its envelope.

        Notes:
            The envelope and the request buffer are always freed.

        Args:
            symbol: The exported C symbol to call.
            args_obj: The wire-cased plain-parameter object (or ``None`` /
                ``{}`` when there are none).
            slices: ``(array_like, dtype_str)`` pairs in declaration order;
                each is made C-contiguous with the exact dtype and passed
                as ``(ptr, element_count)``.
            handle: For methods; passed as the leading ``u64``.

        Returns:
            The decoded JSON payload with every ``__rspyts_buf__``
            placeholder replaced by a numpy array (status 0).

        Raises:
            BridgeError: The registered subclass for the error code
                (status 1), or :class:`~rspyts.errors.RspytsPanicError`
                (status 2).
            ValueError: If a plain argument carries a non-finite float.
        """
        cdll = self.load()
        encoded = envelope.build_request(args_obj)

        # Rust receives shared slices while ctypes releases the GIL. Always
        # copy into runtime-owned, naturally aligned storage so another
        # Python or native thread cannot mutate an aliased writable ndarray
        # during the call. The list keeps those private arrays alive until
        # the foreign function returns.
        arrays = [
            np.require(data, dtype=envelope.DTYPES[dt], requirements=("C", "A", "O")).copy(order="C")
            for data, dt in slices
        ]

        # Per-symbol C signatures vary (handle presence, slice count), so
        # arguments are wrapped in exact ctypes values per call instead of
        # declaring static argtypes on the shared function object.
        fn = getattr(cdll, symbol)
        fn.restype = ctypes.c_void_p

        c_args: list[typing.Any] = []
        if handle is not None:
            c_args.append(ctypes.c_uint64(handle))

        request_len = len(encoded)
        request_ptr = None
        if request_len:
            request_ptr = cdll.rspyts_alloc(request_len)
            ctypes.memmove(request_ptr, encoded, request_len)
        # A null pointer with len 0 is valid: the shim reads len == 0 as "{}".
        c_args.append(ctypes.c_void_p(request_ptr))
        c_args.append(ctypes.c_size_t(request_len))

        for arr in arrays:
            c_args.append(ctypes.c_void_p(arr.ctypes.data))
            c_args.append(ctypes.c_size_t(arr.size))

        try:
            ret = fn(*c_args)
        finally:
            if request_ptr is not None:
                cdll.rspyts_free(ctypes.c_void_p(request_ptr), ctypes.c_size_t(request_len))
        if ret is None:
            raise RuntimeError(f"rspyts: {symbol} returned a null envelope")

        # Read the fixed-size header in place first. This does not allocate a
        # Python bytes object, so once `ret` is accepted we can always derive
        # the exact length required by rspyts_free.
        header = (ctypes.c_ubyte * envelope.HEADER_LEN).from_address(ret)
        json_len = sum(int(header[4 + index]) << (8 * index) for index in range(4))
        tail_len = sum(int(header[8 + index]) << (8 * index) for index in range(4))
        total = envelope.HEADER_LEN + json_len + tail_len
        try:
            raw = ctypes.string_at(ret, total)
        finally:
            # Even malformed content or a host-side allocation failure must
            # release the response with the header-derived exact length.
            cdll.rspyts_free(ctypes.c_void_p(ret), ctypes.c_size_t(total))

        status, payload = envelope.parse_envelope(raw)
        if status == 0:
            return payload
        errors.raise_bridge_error(status, payload, error_types)

    def call_drop(self, symbol: str, handle: int) -> None:
        """
        Invoke a ``__drop`` export: fire-and-forget, no envelope (ABI §8).

        Args:
            symbol: The exported drop symbol.
            handle: The handle to release.
        """
        cdll = self.load()
        fn = getattr(cdll, symbol)
        fn.restype = None
        fn(ctypes.c_uint64(handle))
