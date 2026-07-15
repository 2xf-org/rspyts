"""Build and validate native artifacts for rspyts-backed Python wheels.

This module deliberately has no Hatch imports.  The small adapter in
``rspyts.hatch`` translates Hatch lifecycle calls into these independently
testable operations.
"""

from __future__ import annotations

import importlib
import json
import os
import pathlib
import shutil
import subprocess
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any, cast

from packaging import tags

BUILD_REPORT_VERSION = 1
LINUX_SYSTEM_LIBRARIES = frozenset(
    {
        "ld-linux-aarch64.so.1",
        "ld-linux-armhf.so.3",
        "ld-linux-x86-64.so.2",
        "ld-linux.so.2",
        "libc.so",
        "libc.so.6",
        "libdl.so.2",
        "libgcc_s.so.1",
        "libm.so.6",
        "libpthread.so.0",
        "libresolv.so.2",
        "librt.so.1",
        "libutil.so.1",
    }
)


@dataclass(frozen=True)
class NativeBuild:
    """One validated host cdylib and its generated Python destination."""

    artifact: pathlib.Path
    target: str
    python_out: pathlib.Path
    platform_tag: str

    @property
    def library_dir(self) -> pathlib.Path:
        """Return the package-local directory searched by generated code."""
        return self.python_out / "lib"


def resolve_hook_config(project_root: pathlib.Path, config: dict[str, object]) -> pathlib.Path:
    """Validate the intentionally one-setting hook configuration."""
    if set(config) != {"config"}:
        raise RuntimeError('The rspyts Hatch hook accepts only `config = "path/to/rspyts.toml"`')
    value = config["config"]
    if not isinstance(value, str) or not value:
        raise RuntimeError("The rspyts Hatch hook `config` setting must be a non-empty string")
    path = (project_root / value).resolve()
    if not path.is_file():
        raise RuntimeError(f"rspyts configuration does not exist: {path}")
    return path


def build_native(
    *,
    project_root: pathlib.Path,
    config_path: pathlib.Path,
    build_directory: pathlib.Path,
    runtime_version: str,
    editable: bool,
) -> NativeBuild:
    """Check generated code, build one host cdylib, and validate the result."""
    project_root = project_root.resolve()
    config_path = config_path.resolve()
    verify_cli_version(project_root, runtime_version)
    run_cli(
        ["rspyts", "check", "--config", str(config_path), "--locked"],
        cwd=project_root,
        purpose="check generated bindings",
    )

    command = [
        "rspyts",
        "build",
        "--config",
        str(config_path),
        "--target",
        "host",
    ]
    stage: pathlib.Path | None = None
    if editable:
        expected_profile = "dev"
    else:
        expected_profile = "release"
        stage = wheel_stage_dir(project_root, build_directory)
        cleanup_wheel_stage(project_root, build_directory)
        stage.mkdir(parents=True)
        command.extend(["--out-dir", str(stage), "--release"])
    command.extend(["--locked", "--output-format", "json"])

    try:
        result = run_cli(command, cwd=project_root, purpose="build the native library")
        build = parse_build_report(
            result.stdout,
            project_root=project_root,
            expected_profile=expected_profile,
            stage=stage,
            editable=editable,
        )
        if not editable:
            if target_family(build.target) == "macos":
                normalize_macos_dylib(build.artifact)
            validate_system_dependencies(build.artifact, build.target)
        return build
    except Exception:
        if stage is not None:
            cleanup_wheel_stage(project_root, build_directory)
        raise


def configure_standard_wheel(build_data: dict[str, object], build: NativeBuild, project_root: pathlib.Path) -> None:
    """Add a validated cdylib and honest non-pure tag to Hatch build data."""
    force_include = build_data.setdefault("force_include", {})
    if not isinstance(force_include, dict):
        raise RuntimeError("Hatch build_data.force_include must be a mapping")
    force_include = cast(dict[str, object], force_include)
    source_root = (project_root / "src").resolve()
    destination = build.library_dir.relative_to(source_root) / build.artifact.name
    force_include[str(build.artifact)] = destination.as_posix()
    build_data["pure_python"] = False
    build_data["tag"] = f"py3-none-{build.platform_tag}"


def verify_cli_version(project_root: pathlib.Path, runtime_version: str) -> None:
    """Require the installed CLI and Python runtime to have the same version."""
    result = run_cli(["rspyts", "--version"], cwd=project_root, purpose="read the CLI version")
    expected = f"rspyts {runtime_version}"
    actual = result.stdout.strip()
    if actual != expected:
        raise RuntimeError(
            f"rspyts CLI/runtime version mismatch: CLI reported {actual!r}, runtime is {runtime_version!r}"
        )


def run_cli(arguments: list[str], *, cwd: pathlib.Path, purpose: str) -> subprocess.CompletedProcess[str]:
    """Run one rspyts command and turn process failures into build errors."""
    try:
        result = subprocess.run(arguments, cwd=cwd, check=False, capture_output=True, text=True)
    except FileNotFoundError as error:
        raise RuntimeError("Cannot run `rspyts`; install the matching rspyts CLI") from error
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        raise RuntimeError(f"rspyts failed to {purpose}: {detail}")
    return result


def parse_build_report(
    text: str,
    *,
    project_root: pathlib.Path,
    expected_profile: str,
    stage: pathlib.Path | None,
    editable: bool,
) -> NativeBuild:
    """Validate the versioned report emitted by ``rspyts build``."""
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        raise RuntimeError("rspyts returned a malformed JSON build report") from error
    report = require_object(value, "build report")
    if type(report.get("formatVersion")) is not int or report["formatVersion"] != BUILD_REPORT_VERSION:
        raise RuntimeError(f"Unsupported rspyts build report format: {report.get('formatVersion')!r}")

    crate = require_object(report.get("crate"), "build report crate")
    require_string(crate.get("name"), "build report crate.name")
    require_string(crate.get("version"), "build report crate.version")

    resolved = require_object(report.get("build"), "build report build")
    if resolved.get("profile") != expected_profile or resolved.get("locked") is not True:
        raise RuntimeError(f"rspyts build report is not a locked {expected_profile} build")

    python = require_object(report.get("python"), "build report python")
    python_out = pathlib.Path(require_string(python.get("out"), "build report python.out")).resolve()
    source_root = (project_root / "src").resolve()
    if not python_out.is_relative_to(source_root):
        raise RuntimeError(f"rspyts Python output escapes the project source tree: {python_out}")

    artifacts = report.get("artifacts")
    if not isinstance(artifacts, list):
        raise RuntimeError("rspyts build report artifacts must be a list")
    if len(artifacts) != 1:
        raise RuntimeError(f"Expected exactly one native rspyts artifact, found {len(artifacts)}")
    artifact_report = require_object(artifacts[0], "build report artifact")
    if artifact_report.get("kind") != "native":
        raise RuntimeError("The rspyts Hatch hook requires one host native artifact")
    target = require_string(artifact_report.get("target"), "build report artifact.target")
    artifact = pathlib.Path(require_string(artifact_report.get("path"), "build report artifact.path")).resolve()
    if not artifact.is_file():
        raise RuntimeError(f"rspyts native artifact does not exist: {artifact}")
    validate_library_suffix(artifact, target)

    if editable:
        expected = python_out / "lib" / artifact.name
        if artifact != expected:
            raise RuntimeError(
                f"rspyts did not atomically stage the editable native library at {expected}: reported {artifact}"
            )
    elif stage is None or artifact.parent != stage.resolve():
        raise RuntimeError(f"rspyts staged the wheel artifact outside the temporary build directory: {artifact}")

    return NativeBuild(
        artifact=artifact,
        target=target,
        python_out=python_out,
        platform_tag=find_platform_tag(target, artifact),
    )


def require_object(value: object, label: str) -> dict[str, object]:
    """Require a JSON object in a build report."""
    if not isinstance(value, dict):
        raise RuntimeError(f"{label} must be an object")
    return cast(dict[str, object], value)


def require_string(value: object, label: str) -> str:
    """Require a non-empty JSON string in a build report."""
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"{label} must be a non-empty string")
    return value


def wheel_stage_dir(project_root: pathlib.Path, build_directory: pathlib.Path) -> pathlib.Path:
    """Return the source-independent temporary native wheel directory."""
    directory = build_directory if build_directory.is_absolute() else project_root / build_directory
    return directory.resolve() / ".rspyts-native"


def cleanup_wheel_stage(project_root: pathlib.Path, build_directory: pathlib.Path) -> None:
    """Remove only the hook-owned temporary native wheel directory."""
    stage = wheel_stage_dir(project_root, build_directory)
    if stage.exists():
        shutil.rmtree(stage)


def find_platform_tag(target: str, artifact: pathlib.Path, supported: Iterable[tags.Tag] | None = None) -> str:
    """Select an interpreter-supported platform tag matching the host cdylib."""
    rust_arch = target_arch(target)
    family = target_family(target)
    candidates = list(supported if supported is not None else tags.sys_tags())
    if family == "linux":
        policy = linux_policy_prefix(target)
        for tag in candidates:
            if tag.platform.startswith(policy) and platform_arch(tag.platform) == rust_arch:
                return tag.platform
        raise RuntimeError(f"No interpreter-supported {policy.removesuffix('_')} policy matches Rust target {target!r}")

    if family == "macos":
        architectures = read_macos_architectures(artifact)
        if architectures != {rust_arch}:
            actual = ", ".join(sorted(architectures)) or "none"
            raise RuntimeError(
                f"Mach-O architectures are {actual}, expected one {rust_arch} slice for Rust target {target!r}"
            )
        deployment = read_macos_deployment_target(artifact)
        platform = f"macosx_{deployment[0]}_{deployment[1]}_{rust_arch}"
        if platform not in {tag.platform for tag in candidates}:
            raise RuntimeError(
                f"The interpreter does not support artifact deployment tag {platform!r} for Rust target {target!r}"
            )
        return platform

    for tag in candidates:
        platform = tag.platform
        if "universal2" in platform:
            continue
        try:
            same_platform = platform_family(platform) == family and platform_arch(platform) == rust_arch
        except RuntimeError:
            continue
        if same_platform:
            return platform
    raise RuntimeError(f"No interpreter-supported platform wheel tag matches Rust target {target!r}")


def linux_policy_prefix(target: str) -> str:
    """Map a Linux Rust target to the only honest wheel policy family."""
    if "linux-gnu" in target:
        return "manylinux_"
    if "linux-musl" in target:
        return "musllinux_"
    raise RuntimeError(f"Rust Linux target does not identify a supported glibc or musl ABI: {target!r}")


def normalize_macos_dylib(artifact: pathlib.Path) -> None:
    """Atomically set and verify a relocatable LC_ID_DYLIB on every slice."""
    temporary = artifact.with_name(f".{artifact.name}.rspyts.tmp")
    identity = f"@rpath/{artifact.name}"
    try:
        shutil.copy2(artifact, temporary)
        run_tool(
            ["install_name_tool", "-id", identity, str(temporary)],
            missing="Cannot normalize the macOS dylib because `install_name_tool` is unavailable",
        )
        identities = read_macos_dylib_identities(temporary)
        if identities != {identity}:
            actual = ", ".join(sorted(identities)) or "none"
            raise RuntimeError(f"Normalized macOS dylib identity is {actual}, expected {identity}")
        temporary.replace(artifact)
    finally:
        temporary.unlink(missing_ok=True)


def validate_system_dependencies(artifact: pathlib.Path, target: str) -> None:
    """Fail closed when a cdylib needs libraries a platform wheel will not contain."""
    family = target_family(target)
    if family == "linux":
        dependencies = read_elf_dependencies(artifact)
        external = dependencies - LINUX_SYSTEM_LIBRARIES
    elif family == "macos":
        dependencies = read_macos_dependencies(artifact)
        identity = f"@rpath/{artifact.name}"
        external = {
            dependency
            for dependency in dependencies
            if dependency != identity
            and not dependency.startswith("/usr/lib/")
            and not dependency.startswith("/System/Library/")
        }
    else:
        dependencies = read_windows_dependencies(artifact)
        external = {dependency for dependency in dependencies if not is_windows_system_dependency(dependency)}
    if external:
        names = ", ".join(sorted(external))
        raise RuntimeError(
            "rspyts native wheels must be self-contained apart from operating-system libraries; "
            f"{artifact.name} requires {names}. Bundle or statically link those libraries, or use "
            "auditwheel, delocate, or delvewheel in a dedicated packaging pipeline."
        )


def read_elf_dependencies(artifact: pathlib.Path) -> set[str]:
    """Read ELF DT_NEEDED entries without depending on a system binutils install."""
    try:
        elffile = cast(Any, importlib.import_module("elftools.elf.elffile"))
    except ImportError as error:
        raise RuntimeError("Cannot audit Linux dependencies; install the `rspyts[hatch]` extra") from error
    try:
        with artifact.open("rb") as stream:
            dynamic = elffile.ELFFile(stream).get_section_by_name(".dynamic")
            if dynamic is None:
                raise RuntimeError(f"ELF artifact has no dynamic section: {artifact}")
            return {str(tag.needed) for tag in dynamic.iter_tags() if tag.entry.d_tag == "DT_NEEDED"}
    except RuntimeError:
        raise
    except Exception as error:
        raise RuntimeError(f"Cannot inspect ELF dependencies for {artifact}: {error}") from error


def read_macos_dependencies(artifact: pathlib.Path) -> set[str]:
    """Read Mach-O load dependencies from ``otool -L``."""
    result = run_tool(
        ["otool", "-L", str(artifact)],
        missing="Cannot audit macOS dependencies because `otool` is unavailable",
    )
    dependencies = {
        line.split(" (compatibility version", maxsplit=1)[0]
        for raw in result.stdout.splitlines()[1:]
        if (line := raw.strip())
    }
    if not dependencies:
        raise RuntimeError(f"otool reported no dynamic-library entries for {artifact}")
    return dependencies


def read_windows_dependencies(artifact: pathlib.Path) -> set[str]:
    """Read PE import tables without depending on Visual Studio tooling."""
    try:
        pefile = cast(Any, importlib.import_module("pefile"))
    except ImportError as error:
        raise RuntimeError("Cannot audit Windows dependencies; install the `rspyts[hatch]` extra") from error
    try:
        image = pefile.PE(str(artifact), fast_load=True)
        try:
            image.parse_data_directories(
                directories=[
                    pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
                    pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"],
                ]
            )
            entries = [
                *getattr(image, "DIRECTORY_ENTRY_IMPORT", []),
                *getattr(image, "DIRECTORY_ENTRY_DELAY_IMPORT", []),
            ]
            return {entry.dll.decode("ascii").lower() for entry in entries}
        finally:
            image.close()
    except Exception as error:
        raise RuntimeError(f"Cannot inspect Windows dependencies for {artifact}: {error}") from error


def is_windows_system_dependency(name: str) -> bool:
    """Return whether a PE import is supplied by the Windows system directory."""
    normalized = name.lower()
    if normalized.startswith(("api-ms-win-", "ext-ms-win-")):
        return True
    system_root = pathlib.Path(os.environ.get("SystemRoot", r"C:\Windows"))
    return (system_root / "System32" / normalized).is_file()


def read_macos_dylib_identities(artifact: pathlib.Path) -> set[str]:
    """Read the LC_ID_DYLIB values reported for all Mach-O slices."""
    result = run_tool(
        ["otool", "-D", str(artifact)],
        missing="Cannot verify the macOS dylib identity because `otool` is unavailable",
    )
    identities = {line for raw in result.stdout.splitlines() if (line := raw.strip()) and not line.endswith(":")}
    if not identities:
        raise RuntimeError(f"otool reported no dynamic-library identity for {artifact}")
    return identities


def read_macos_architectures(artifact: pathlib.Path) -> set[str]:
    """Read the exact Mach-O slice architectures with ``lipo``."""
    result = run_tool(
        ["lipo", "-archs", str(artifact)],
        missing="Cannot inspect the macOS dylib architectures because `lipo` is unavailable",
    )
    architectures = set(result.stdout.split())
    if not architectures:
        raise RuntimeError(f"lipo reported no Mach-O architectures for {artifact}")
    return architectures


def read_macos_deployment_target(artifact: pathlib.Path) -> tuple[int, int]:
    """Read the common minimum macOS version from all Mach-O slices."""
    result = run_tool(
        ["otool", "-l", str(artifact)],
        missing="Cannot inspect the macOS dylib deployment target because `otool` is unavailable",
    )
    return parse_macos_deployment_target(result.stdout)


def run_tool(command: list[str], *, missing: str) -> subprocess.CompletedProcess[str]:
    """Run one required platform inspection or mutation tool."""
    try:
        result = subprocess.run(command, check=False, capture_output=True, text=True)
    except FileNotFoundError as error:
        raise RuntimeError(missing) from error
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        raise RuntimeError(f"{command[0]} failed for {command[-1]}: {detail}")
    return result


def parse_macos_deployment_target(text: str) -> tuple[int, int]:
    """Parse modern and legacy deployment commands from ``otool -l``."""
    command: str | None = None
    platform: str | None = None
    versions: list[tuple[int, int]] = []
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("cmd "):
            command = line.removeprefix("cmd ").strip()
            platform = None
        elif command == "LC_BUILD_VERSION" and line.startswith("platform "):
            platform = line.removeprefix("platform ").strip()
        elif command == "LC_BUILD_VERSION" and line.startswith("minos "):
            if platform not in {"1", "MACOS"}:
                raise RuntimeError(f"LC_BUILD_VERSION does not identify the artifact as macOS: {platform!r}")
            versions.append(parse_macos_version(line.removeprefix("minos ").strip()))
            command = None
        elif command == "LC_VERSION_MIN_MACOSX" and line.startswith("version "):
            versions.append(parse_macos_version(line.removeprefix("version ").strip()))
            command = None
    unique = set(versions)
    if not unique:
        raise RuntimeError("otool reported no LC_BUILD_VERSION or LC_VERSION_MIN_MACOSX deployment target")
    if len(unique) != 1:
        values = ", ".join(f"{major}.{minor}" for major, minor in sorted(unique))
        raise RuntimeError(f"Mach-O slices disagree on macOS deployment target: {values}")
    return unique.pop()


def parse_macos_version(value: str) -> tuple[int, int]:
    """Normalize a two- or three-component deployment version."""
    parts = value.split(".")
    if len(parts) not in {2, 3} or not all(part.isdigit() for part in parts):
        raise RuntimeError(f"Invalid macOS deployment target from otool: {value!r}")
    return int(parts[0]), int(parts[1])


def target_family(target: str) -> str:
    """Normalize a supported Rust target operating-system family."""
    if "apple-darwin" in target:
        return "macos"
    if "windows" in target:
        return "windows"
    if "linux" in target:
        return "linux"
    raise RuntimeError(f"Unsupported native Rust target: {target!r}")


def target_arch(target: str) -> str:
    """Normalize a supported Rust target architecture."""
    arch = target.split("-", maxsplit=1)[0]
    aliases = {
        "aarch64": "arm64",
        "armv7": "armv7l",
        "i386": "x86",
        "i586": "x86",
        "i686": "x86",
        "powerpc64le": "ppc64le",
        "x86_64": "x86_64",
    }
    normalized = aliases.get(arch, arch if arch in {"ppc64", "s390x"} else None)
    if normalized is None:
        raise RuntimeError(f"Unsupported Rust target architecture: {target!r}")
    return normalized


def platform_family(platform: str) -> str:
    """Normalize a supported wheel platform family."""
    if platform.startswith("macosx_"):
        return "macos"
    if platform.startswith("win"):
        return "windows"
    raise RuntimeError(f"Unsupported wheel platform tag: {platform!r}")


def platform_arch(platform: str) -> str:
    """Normalize a supported wheel platform architecture."""
    if platform == "win32":
        return "x86"
    suffixes = {
        "_aarch64": "arm64",
        "_amd64": "x86_64",
        "_arm64": "arm64",
        "_armv7l": "armv7l",
        "_i686": "x86",
        "_ppc64": "ppc64",
        "_ppc64le": "ppc64le",
        "_s390x": "s390x",
        "_x86_64": "x86_64",
    }
    for suffix, arch in suffixes.items():
        if platform.endswith(suffix):
            return arch
    raise RuntimeError(f"Unsupported wheel platform architecture: {platform!r}")


def validate_library_suffix(path: pathlib.Path, target: str) -> None:
    """Require the dynamic-library suffix for the reported host target."""
    expected = {"macos": ".dylib", "windows": ".dll", "linux": ".so"}[target_family(target)]
    if path.suffix != expected:
        raise RuntimeError(f"Native artifact {path} has suffix {path.suffix!r}, expected {expected!r} for {target}")
