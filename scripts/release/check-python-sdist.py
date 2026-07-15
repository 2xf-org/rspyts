#!/usr/bin/env python3
"""Verify that a Python sdist contains exactly the intended source files."""

from __future__ import annotations

import argparse
import sys
import tarfile
from pathlib import Path, PurePosixPath


def expected_files(package_root: Path) -> set[str]:
    expected = {"LICENSE", "PKG-INFO", "README.md", "pyproject.toml"}
    for directory in ("src", "tests"):
        for path in (package_root / directory).rglob("*"):
            if (
                not path.is_file()
                or "__pycache__" in path.parts
                or path.suffix == ".pyc"
            ):
                continue
            expected.add(path.relative_to(package_root).as_posix())
    return expected


def archive_files(sdist: Path) -> set[str]:
    files: set[str] = set()
    roots: set[str] = set()
    with tarfile.open(sdist, "r:gz") as archive:
        for member in archive.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts:
                raise ValueError(f"unsafe sdist member: {member.name}")
            roots.add(path.parts[0])
            if member.isdir():
                continue
            if not member.isfile():
                raise ValueError(f"unsupported sdist member: {member.name}")
            if len(path.parts) < 2:
                raise ValueError(f"file outside the sdist root: {member.name}")
            relative = PurePosixPath(*path.parts[1:]).as_posix()
            if relative in files:
                raise ValueError(f"duplicate sdist member: {relative}")
            files.add(relative)
    if len(roots) != 1:
        raise ValueError(
            f"sdist must have one top-level directory, found: {sorted(roots)}"
        )
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("sdist", type=Path)
    parser.add_argument("package_root", type=Path)
    args = parser.parse_args()

    expected = expected_files(args.package_root.resolve())
    try:
        actual = archive_files(args.sdist.resolve())
    except (OSError, tarfile.TarError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1

    missing = sorted(expected - actual)
    unexpected = sorted(actual - expected)
    if missing or unexpected:
        if missing:
            print(
                "missing from sdist:",
                *(f"  {path}" for path in missing),
                sep="\n",
                file=sys.stderr,
            )
        if unexpected:
            print(
                "unexpected in sdist:",
                *(f"  {path}" for path in unexpected),
                sep="\n",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
