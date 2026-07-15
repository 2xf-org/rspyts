#!/usr/bin/env python3
"""Build and exercise one real Hatch-backed native wheel on the host."""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]


def run(command: list[str], *, cwd: pathlib.Path | None = None, env: dict[str, str] | None = None) -> None:
    subprocess.run(command, cwd=cwd, env=env, check=True)


def venv_python(environment: pathlib.Path) -> pathlib.Path:
    directory = "Scripts" if os.name == "nt" else "bin"
    executable = "python.exe" if os.name == "nt" else "python"
    return environment / directory / executable


def copy_fixture(destination: pathlib.Path) -> pathlib.Path:
    for name in ["Cargo.toml"]:
        shutil.copy2(ROOT / name, destination / name)

    manifest = (destination / "Cargo.toml").read_text()
    for member in [
        '    "crates/rspyts-cli",\n',
        '    "examples/multi-crate/shared/rust",\n',
        '    "examples/multi-crate/app/rust",\n',
    ]:
        if member not in manifest:
            raise RuntimeError(f"workspace manifest no longer contains expected member {member.strip()}")
        manifest = manifest.replace(member, "")
    (destination / "Cargo.toml").write_text(manifest)

    ignore = shutil.ignore_patterns(
        ".coverage",
        ".pytest_cache",
        ".ruff_cache",
        ".venv",
        "__pycache__",
        "dist",
        "lib",
        "node_modules",
        "target",
    )
    for relative in [
        pathlib.Path("crates/rspyts-core"),
        pathlib.Path("crates/rspyts-macros"),
        pathlib.Path("crates/rspyts"),
        pathlib.Path("examples/basic"),
    ]:
        shutil.copytree(ROOT / relative, destination / relative, ignore=ignore)

    project = destination / "examples/basic/python"
    pyproject = project / "pyproject.toml"
    text = pyproject.read_text()
    source = '[tool.uv.sources]\nrspyts = { path = "../../../runtimes/python", editable = true }\n\n'
    if source not in text or 'requires = ["hatchling"]' not in text:
        raise RuntimeError("basic example Python metadata no longer matches the native-wheel fixture")
    text = text.replace(source, "")
    text = text.replace(
        'requires = ["hatchling"]',
        'requires = ["hatchling>=1.27,<2.0", "rspyts[hatch]==0.3.2"]',
    )
    text += '\n[tool.hatch.build.hooks.rspyts]\nconfig = "../rspyts.toml"\n'
    pyproject.write_text(text)
    return project


def verify_wheel(wheel: pathlib.Path) -> None:
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()
        metadata = [name for name in names if name.endswith(".dist-info/WHEEL")]
        native = [name for name in names if "/generated/lib/" in name and name.endswith((".dll", ".dylib", ".so"))]
        if len(metadata) != 1 or len(native) != 1:
            raise RuntimeError(
                f"native wheel must contain one WHEEL metadata file and one cdylib: {metadata=}, {native=}"
            )
        wheel_metadata = archive.read(metadata[0]).decode()
    if "Root-Is-Purelib: false" not in wheel_metadata:
        raise RuntimeError("native wheel incorrectly declares itself pure Python")
    tags = [line.removeprefix("Tag: ") for line in wheel_metadata.splitlines() if line.startswith("Tag: ")]
    if len(tags) != 1 or tags[0].endswith("-any"):
        raise RuntimeError(f"native wheel must declare one platform tag, got {tags}")


def main() -> None:
    if shutil.which("rspyts") is None:
        raise RuntimeError("install the workspace rspyts CLI before running the native-wheel smoke")

    with tempfile.TemporaryDirectory(prefix="rspyts-native-wheel-") as temporary:
        root = pathlib.Path(temporary)
        fixture = root / "fixture"
        fixture.mkdir()
        project = copy_fixture(fixture)
        run(["cargo", "generate-lockfile"], cwd=fixture)

        build_environment = root / "build-env"
        run(["uv", "venv", "--python", "3.11", str(build_environment)])
        build_python = venv_python(build_environment)
        run(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(build_python),
                f"{ROOT / 'runtimes/python'}[hatch]",
            ]
        )

        wheel_dir = root / "wheel"
        environment = os.environ.copy()
        environment["VIRTUAL_ENV"] = str(build_environment)
        run(
            [
                "uv",
                "build",
                "--wheel",
                "--no-build-isolation",
                "--python",
                str(build_python),
                "--out-dir",
                str(wheel_dir),
                str(project),
            ],
            cwd=fixture,
            env=environment,
        )
        wheels = list(wheel_dir.glob("*.whl"))
        if len(wheels) != 1:
            raise RuntimeError(f"expected one native wheel, found {wheels}")
        verify_wheel(wheels[0])

        runtime_dir = root / "runtime"
        run(["uv", "build", "--wheel", "--out-dir", str(runtime_dir), str(ROOT / "runtimes/python")])
        runtime_wheels = list(runtime_dir.glob("*.whl"))
        if len(runtime_wheels) != 1:
            raise RuntimeError(f"expected one runtime wheel, found {runtime_wheels}")

        consumer = root / "consumer"
        run(["uv", "venv", "--python", "3.11", str(consumer)])
        consumer_python = venv_python(consumer)
        run(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(consumer_python),
                str(runtime_wheels[0]),
                str(wheels[0]),
            ]
        )
        code = (
            "import basic_example as example; "
            "result = example.summarize([2.0, 4.0], None); "
            "assert result.average == 3.0"
        )
        consumer_environment = os.environ.copy()
        consumer_environment.pop("RSPYTS_LIBRARY_BASIC_EXAMPLE", None)
        run([str(consumer_python), "-c", code], env=consumer_environment)


if __name__ == "__main__":
    main()
