#!/usr/bin/env python3

"""Compare Cargo archives across an internal dependency publication boundary."""

from __future__ import annotations

import argparse
import re
import tarfile
import tomllib
from pathlib import Path, PurePosixPath


REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
CHECKSUM = re.compile(r"[0-9a-f]{64}")


class ComparisonError(ValueError):
    """Raised when two crate archives are not an approved semantic match."""


def archive_files(path: Path) -> dict[PurePosixPath, bytes]:
    """Read regular files from a crate archive after validating their paths."""

    files: dict[PurePosixPath, bytes] = {}
    with tarfile.open(path, mode="r:gz") as archive:
        for member in archive.getmembers():
            member_path = PurePosixPath(member.name)
            if member_path.is_absolute() or ".." in member_path.parts:
                raise ComparisonError(f"unsafe archive path in {path}: {member.name}")
            if member.isdir():
                continue
            if not member.isfile():
                raise ComparisonError(
                    f"unsupported archive member in {path}: {member.name}"
                )
            extracted = archive.extractfile(member)
            if extracted is None:
                raise ComparisonError(f"could not read {member.name} from {path}")
            if member_path in files:
                raise ComparisonError(
                    f"duplicate archive member in {path}: {member.name}"
                )
            files[member_path] = extracted.read()
    if not files:
        raise ComparisonError(f"crate archive is empty: {path}")
    return files


def normalized_lock(
    raw: bytes,
    allowed_dependencies: dict[tuple[str, str], str],
    archive: Path,
) -> dict[str, object]:
    """Remove only verified registry provenance for allowed internal packages."""

    try:
        lock = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ComparisonError(f"invalid Cargo.lock in {archive}: {error}") from error

    packages = lock.get("package")
    if not isinstance(packages, list):
        raise ComparisonError(f"Cargo.lock in {archive} has no package list")

    for (name, version), expected_checksum in allowed_dependencies.items():
        matches = [
            package
            for package in packages
            if isinstance(package, dict)
            and package.get("name") == name
            and package.get("version") == version
        ]
        if len(matches) != 1:
            raise ComparisonError(
                f"Cargo.lock in {archive} must contain exactly one {name} {version}"
            )
        package = matches[0]
        source = package.pop("source", None)
        checksum = package.pop("checksum", None)
        if source is None and checksum is None:
            continue
        if source != REGISTRY_SOURCE or checksum != expected_checksum:
            raise ComparisonError(
                f"Cargo.lock in {archive} has unexpected provenance "
                f"for {name} {version}"
            )

    return lock


def compare_archives(
    candidate: Path,
    repacked: Path,
    allowed_dependencies: dict[tuple[str, str], str],
) -> None:
    """Require exact files except approved Cargo.lock registry provenance."""

    candidate_files = archive_files(candidate)
    repacked_files = archive_files(repacked)
    if candidate_files.keys() != repacked_files.keys():
        missing = sorted(
            str(path) for path in candidate_files.keys() - repacked_files.keys()
        )
        added = sorted(
            str(path) for path in repacked_files.keys() - candidate_files.keys()
        )
        raise ComparisonError(
            f"crate file set changed; missing={missing}, added={added}"
        )

    lock_paths = [path for path in candidate_files if path.name == "Cargo.lock"]
    if len(lock_paths) != 1:
        raise ComparisonError("crate archives must contain exactly one Cargo.lock")
    lock_path = lock_paths[0]

    for path, candidate_bytes in candidate_files.items():
        if path == lock_path:
            continue
        if candidate_bytes != repacked_files[path]:
            raise ComparisonError(f"crate content changed outside Cargo.lock: {path}")

    candidate_lock = normalized_lock(
        candidate_files[lock_path], allowed_dependencies, candidate
    )
    repacked_lock = normalized_lock(
        repacked_files[lock_path], allowed_dependencies, repacked
    )
    if candidate_lock != repacked_lock:
        raise ComparisonError("Cargo.lock changed beyond approved registry provenance")


def dependency(value: str) -> tuple[tuple[str, str], str]:
    """Parse NAME@VERSION=CHECKSUM."""

    identity, separator, checksum = value.rpartition("=")
    name, at, version = identity.rpartition("@")
    if (
        not separator
        or not at
        or not name
        or not version
        or not CHECKSUM.fullmatch(checksum)
    ):
        raise argparse.ArgumentTypeError(
            "registry dependencies use NAME@VERSION=64-CHAR-LOWERCASE-SHA256"
        )
    return (name, version), checksum


def main() -> None:
    """Run the command-line archive comparison."""

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("repacked", type=Path)
    parser.add_argument(
        "--registry-dependency",
        action="append",
        default=[],
        type=dependency,
        metavar="NAME@VERSION=CHECKSUM",
    )
    arguments = parser.parse_args()
    allowed_dependencies = dict(arguments.registry_dependency)
    if len(allowed_dependencies) != len(arguments.registry_dependency):
        parser.error("registry dependencies must not be repeated")

    try:
        compare_archives(
            arguments.candidate,
            arguments.repacked,
            allowed_dependencies,
        )
    except ComparisonError as error:
        parser.exit(1, f"crate archives differ: {error}\n")
    print("crate archives are semantically identical")


if __name__ == "__main__":
    main()
