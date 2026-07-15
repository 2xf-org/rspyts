"""Tests for native wheel construction without Hatch coupling."""

from __future__ import annotations

import json
import pathlib
import subprocess
import unittest.mock
from collections.abc import Callable

import packaging.tags
import pytest

from rspyts import native_wheel


def make_project(tmp_path: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path]:
    project = tmp_path / "demo" / "python"
    output = project / "src/demo/generated"
    output.mkdir(parents=True)
    config = project.parent / "rspyts.toml"
    config.write_text('[crate]\npath = "rust"\n[python]\nout = "python/src/demo/generated"\n')
    return project, config, output


def report(output: pathlib.Path, artifact: pathlib.Path, target: str, profile: str) -> dict[str, object]:
    return {
        "formatVersion": 1,
        "crate": {"name": "demo", "version": "1.2.3"},
        "build": {"features": [], "noDefaultFeatures": False, "profile": profile, "locked": True},
        "python": {"out": str(output)},
        "artifacts": [{"kind": "native", "target": target, "path": str(artifact)}],
    }


def completed(stdout: str = "", stderr: str = "", code: int = 0) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(["rspyts"], code, stdout, stderr)


def test_hook_config_is_one_explicit_existing_path(tmp_path: pathlib.Path) -> None:
    project, config, _ = make_project(tmp_path)
    assert native_wheel.resolve_hook_config(project, {"config": "../rspyts.toml"}) == config.resolve()
    invalid_configs: list[dict[str, object]] = [
        {},
        {"config": "../rspyts.toml", "crate": "demo"},
        {"config": 3},
    ]
    for invalid in invalid_configs:
        with pytest.raises(RuntimeError):
            native_wheel.resolve_hook_config(project, invalid)
    with pytest.raises(RuntimeError, match="does not exist"):
        native_wheel.resolve_hook_config(project, {"config": "missing.toml"})


def test_standard_build_uses_positive_cli_contract_without_source_mutation(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    project, config, output = make_project(tmp_path)
    calls: list[list[str]] = []

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(command)
        assert kwargs["cwd"] == project.resolve()
        if command == ["rspyts", "--version"]:
            return completed("rspyts 0.3.0\n")
        if command[1] == "check":
            return completed()
        stage = pathlib.Path(command[command.index("--out-dir") + 1])
        artifact = stage / "libdemo.so"
        artifact.write_bytes(b"native")
        return completed(json.dumps(report(output, artifact, "x86_64-unknown-linux-gnu", "release")))

    monkeypatch.setattr(native_wheel.subprocess, "run", run)
    monkeypatch.setattr(native_wheel, "find_platform_tag", lambda *_: "manylinux_2_28_x86_64")
    audit = unittest.mock.Mock()
    monkeypatch.setattr(native_wheel, "validate_system_dependencies", audit)

    build = native_wheel.build_native(
        project_root=project,
        config_path=config,
        build_directory=pathlib.Path("dist"),
        runtime_version="0.3.0",
        editable=False,
    )

    stage = project.resolve() / "dist/.rspyts-native"
    assert build.artifact == stage / "libdemo.so"
    assert calls == [
        ["rspyts", "--version"],
        ["rspyts", "check", "--config", str(config.resolve()), "--locked"],
        [
            "rspyts",
            "build",
            "--config",
            str(config.resolve()),
            "--target",
            "host",
            "--out-dir",
            str(stage),
            "--release",
            "--locked",
            "--output-format",
            "json",
        ],
    ]
    assert not (output / "lib").exists()
    audit.assert_called_once_with(build.artifact, build.target)


def test_editable_build_requires_cli_atomic_package_stage(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    project, config, output = make_project(tmp_path)
    calls: list[list[str]] = []

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(command)
        if command == ["rspyts", "--version"] or command[1] == "check":
            return completed("rspyts 0.3.0\n" if len(command) == 2 else "")
        artifact = output / "lib/libdemo.dylib"
        artifact.parent.mkdir()
        artifact.write_bytes(b"native")
        return completed(json.dumps(report(output, artifact, "aarch64-apple-darwin", "dev")))

    monkeypatch.setattr(native_wheel.subprocess, "run", run)
    monkeypatch.setattr(native_wheel, "find_platform_tag", lambda *_: "manylinux_2_28_x86_64")
    audit = unittest.mock.Mock()
    normalize = unittest.mock.Mock()
    monkeypatch.setattr(native_wheel, "validate_system_dependencies", audit)
    monkeypatch.setattr(native_wheel, "normalize_macos_dylib", normalize)
    build = native_wheel.build_native(
        project_root=project,
        config_path=config,
        build_directory=pathlib.Path("dist"),
        runtime_version="0.3.0",
        editable=True,
    )
    assert build.artifact == output.resolve() / "lib/libdemo.dylib"
    assert calls[-1] == [
        "rspyts",
        "build",
        "--config",
        str(config.resolve()),
        "--target",
        "host",
        "--locked",
        "--output-format",
        "json",
    ]
    audit.assert_not_called()
    normalize.assert_not_called()


def test_build_failure_cleans_standard_stage(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    project, config, _ = make_project(tmp_path)
    run = unittest.mock.Mock(
        side_effect=[completed("rspyts 0.3.0\n"), completed(), completed(stderr="cargo failed", code=3)]
    )
    monkeypatch.setattr(native_wheel.subprocess, "run", run)
    with pytest.raises(RuntimeError, match="cargo failed"):
        native_wheel.build_native(
            project_root=project,
            config_path=config,
            build_directory=pathlib.Path("dist"),
            runtime_version="0.3.0",
            editable=False,
        )
    assert not (project / "dist/.rspyts-native").exists()


def test_version_and_process_failures_are_clear(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    project, _, _ = make_project(tmp_path)
    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(return_value=completed("rspyts 0.2.0\n")))
    with pytest.raises(RuntimeError, match="version mismatch"):
        native_wheel.verify_cli_version(project, "0.3.0")
    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(side_effect=FileNotFoundError("rspyts")))
    with pytest.raises(RuntimeError, match="install the matching"):
        native_wheel.run_cli(["rspyts"], cwd=project, purpose="test")


def test_parse_report_and_configure_non_pure_wheel(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    project, _, output = make_project(tmp_path)
    stage = project / "dist/.rspyts-native"
    stage.mkdir(parents=True)
    artifact = stage / "demo.dll"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(native_wheel, "find_platform_tag", lambda *_: "win_amd64")
    build = native_wheel.parse_build_report(
        json.dumps(report(output, artifact, "x86_64-pc-windows-msvc", "release")),
        project_root=project,
        expected_profile="release",
        stage=stage,
        editable=False,
    )
    data: dict[str, object] = {"force_include": {"existing": "file"}}
    native_wheel.configure_standard_wheel(data, build, project)
    assert data == {
        "force_include": {"existing": "file", str(artifact.resolve()): "demo/generated/lib/demo.dll"},
        "pure_python": False,
        "tag": "py3-none-win_amd64",
    }
    with pytest.raises(RuntimeError, match="must be a mapping"):
        native_wheel.configure_standard_wheel({"force_include": []}, build, project)


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda value: "not-json", "malformed"),
        (lambda value: "[]", "must be an object"),
        (lambda value: json.dumps({**value, "formatVersion": 2}), "Unsupported"),
        (lambda value: json.dumps({**value, "crate": []}), "crate must be an object"),
        (lambda value: json.dumps({**value, "build": {"profile": "dev", "locked": True}}), "not a locked release"),
        (lambda value: json.dumps({**value, "artifacts": []}), "exactly one"),
        (
            lambda value: json.dumps({**value, "artifacts": [{"kind": "target", "target": "x", "path": "x"}]}),
            "host native",
        ),
    ],
)
def test_parse_report_rejects_invalid_structure(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
    mutate: Callable[[dict[str, object]], str],
    message: str,
) -> None:
    project, _, output = make_project(tmp_path)
    stage = project / "dist/.rspyts-native"
    stage.mkdir(parents=True)
    artifact = stage / "libdemo.so"
    artifact.write_bytes(b"native")
    value = report(output, artifact, "x86_64-unknown-linux-gnu", "release")
    text = mutate(value)
    monkeypatch.setattr(native_wheel, "find_platform_tag", lambda *_: "manylinux_2_28_x86_64")
    with pytest.raises(RuntimeError, match=message):
        native_wheel.parse_build_report(
            text,
            project_root=project,
            expected_profile="release",
            stage=stage,
            editable=False,
        )


def test_parse_report_rejects_unsafe_or_missing_artifacts(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    project, _, output = make_project(tmp_path)
    stage = project / "dist/.rspyts-native"
    stage.mkdir(parents=True)
    outside = tmp_path / "libdemo.so"
    outside.write_bytes(b"native")
    monkeypatch.setattr(native_wheel, "find_platform_tag", lambda *_: "manylinux_2_28_x86_64")
    with pytest.raises(RuntimeError, match="outside the temporary"):
        native_wheel.parse_build_report(
            json.dumps(report(output, outside, "x86_64-unknown-linux-gnu", "release")),
            project_root=project,
            expected_profile="release",
            stage=stage,
            editable=False,
        )
    outside.unlink()
    with pytest.raises(RuntimeError, match="does not exist"):
        native_wheel.parse_build_report(
            json.dumps(report(output, outside, "x86_64-unknown-linux-gnu", "release")),
            project_root=project,
            expected_profile="release",
            stage=stage,
            editable=False,
        )


@pytest.mark.parametrize(
    ("target", "filename", "platforms", "expected"),
    [
        ("x86_64-unknown-linux-gnu", "libdemo.so", ["manylinux_2_28_x86_64"], "manylinux_2_28_x86_64"),
        ("x86_64-unknown-linux-musl", "libdemo.so", ["musllinux_1_2_x86_64"], "musllinux_1_2_x86_64"),
        ("x86_64-pc-windows-msvc", "demo.dll", ["win_amd64"], "win_amd64"),
    ],
)
def test_platform_tag_matches_target_libc_arch_and_suffix(
    tmp_path: pathlib.Path, target: str, filename: str, platforms: list[str], expected: str
) -> None:
    artifact = tmp_path / filename
    artifact.write_bytes(b"native")
    native_wheel.validate_library_suffix(artifact, target)
    supported = [packaging.tags.Tag("cp311", "cp311", platform) for platform in platforms]
    assert native_wheel.find_platform_tag(target, artifact, supported) == expected


def test_platform_tag_rejects_wrong_libc_arch_family_and_suffix(tmp_path: pathlib.Path) -> None:
    artifact = tmp_path / "libdemo.so"
    artifact.write_bytes(b"native")
    wrong = [packaging.tags.Tag("cp311", "cp311", "musllinux_1_2_x86_64")]
    with pytest.raises(RuntimeError, match="manylinux"):
        native_wheel.find_platform_tag("x86_64-unknown-linux-gnu", artifact, wrong)
    with pytest.raises(RuntimeError, match="glibc or musl"):
        native_wheel.find_platform_tag("x86_64-unknown-linux-uclibc", artifact, wrong)
    with pytest.raises(RuntimeError, match=r"expected '\.dll'"):
        native_wheel.validate_library_suffix(artifact, "x86_64-pc-windows-msvc")
    with pytest.raises(RuntimeError, match="Unsupported native"):
        native_wheel.target_family("wasm32-unknown-unknown")
    with pytest.raises(RuntimeError, match="Unsupported Rust target architecture"):
        native_wheel.target_arch("mips-unknown-linux-gnu")


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("cmd LC_BUILD_VERSION\n platform MACOS\n minos 11.0\n", (11, 0)),
        ("cmd LC_BUILD_VERSION\n platform 1\n minos 12.3.1\n", (12, 3)),
        ("cmd LC_VERSION_MIN_MACOSX\n version 10.9\n", (10, 9)),
    ],
)
def test_parse_macos_deployment_targets(text: str, expected: tuple[int, int]) -> None:
    assert native_wheel.parse_macos_deployment_target(text) == expected


@pytest.mark.parametrize(
    ("text", "message"),
    [
        ("cmd LC_ID_DYLIB\n", "reported no"),
        ("cmd LC_BUILD_VERSION\n platform IOS\n minos 17.0\n", "does not identify"),
        ("cmd LC_VERSION_MIN_MACOSX\n version eleven\n", "Invalid"),
        (
            "cmd LC_BUILD_VERSION\n platform MACOS\n minos 11.0\ncmd LC_VERSION_MIN_MACOSX\n version 12.0\n",
            "slices disagree",
        ),
    ],
)
def test_parse_macos_deployment_rejects_invalid_slices(text: str, message: str) -> None:
    with pytest.raises(RuntimeError, match=message):
        native_wheel.parse_macos_deployment_target(text)


def test_macos_tag_uses_artifact_minos_and_rejects_universal_or_wrong_arch(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    artifact = tmp_path / "libdemo.dylib"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(native_wheel, "read_macos_architectures", lambda _: {"arm64"})
    monkeypatch.setattr(native_wheel, "read_macos_deployment_target", lambda _: (11, 0))
    tags = [
        packaging.tags.Tag("cp311", "cp311", "macosx_14_0_universal2"),
        packaging.tags.Tag("cp311", "cp311", "macosx_14_0_arm64"),
        packaging.tags.Tag("cp311", "cp311", "macosx_11_0_arm64"),
    ]
    assert native_wheel.find_platform_tag("aarch64-apple-darwin", artifact, tags) == "macosx_11_0_arm64"
    with pytest.raises(RuntimeError, match="does not support artifact deployment tag"):
        native_wheel.find_platform_tag(
            "aarch64-apple-darwin",
            artifact,
            [packaging.tags.Tag("cp311", "cp311", "macosx_14_0_x86_64")],
        )

    monkeypatch.setattr(native_wheel, "read_macos_architectures", lambda _: {"arm64", "x86_64"})
    with pytest.raises(RuntimeError, match="expected one arm64 slice"):
        native_wheel.find_platform_tag("aarch64-apple-darwin", artifact, tags)


def test_read_macos_architectures_requires_lipo_output(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    artifact = tmp_path / "libdemo.dylib"
    run = unittest.mock.Mock(return_value=completed("arm64\n"))
    monkeypatch.setattr(native_wheel.subprocess, "run", run)
    assert native_wheel.read_macos_architectures(artifact) == {"arm64"}
    run.assert_called_once_with(["lipo", "-archs", str(artifact)], check=False, capture_output=True, text=True)

    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(return_value=completed()))
    with pytest.raises(RuntimeError, match="reported no Mach-O"):
        native_wheel.read_macos_architectures(artifact)


def test_normalize_macos_dylib_is_atomic_and_verifies_identity(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    artifact = tmp_path / "libdemo.dylib"
    artifact.write_bytes(b"native")
    identity = "@rpath/libdemo.dylib"

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        output = "" if command[0] == "install_name_tool" else f"{command[-1]}:\n{identity}\n"
        return completed(output)

    command = unittest.mock.Mock(side_effect=run)
    monkeypatch.setattr(native_wheel.subprocess, "run", command)
    native_wheel.normalize_macos_dylib(artifact)
    assert artifact.read_bytes() == b"native"
    assert not artifact.with_name(".libdemo.dylib.rspyts.tmp").exists()


def test_macos_tools_report_failures_and_bad_identity(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    artifact = tmp_path / "libdemo.dylib"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(side_effect=FileNotFoundError("otool")))
    with pytest.raises(RuntimeError, match=r"otool.*unavailable"):
        native_wheel.read_macos_deployment_target(artifact)
    monkeypatch.setattr(
        native_wheel.subprocess,
        "run",
        unittest.mock.Mock(side_effect=[completed(), completed(f"{artifact}:\n/absolute/libdemo.dylib\n")]),
    )
    with pytest.raises(RuntimeError, match="expected @rpath"):
        native_wheel.normalize_macos_dylib(artifact)


def test_dependency_audit_accepts_only_system_libraries(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    artifact = tmp_path / "libdemo.so"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(native_wheel, "read_elf_dependencies", lambda _: {"libc.so.6", "libgcc_s.so.1"})
    native_wheel.validate_system_dependencies(artifact, "x86_64-unknown-linux-gnu")

    monkeypatch.setattr(native_wheel, "read_elf_dependencies", lambda _: {"libc.so.6", "libssl.so.3"})
    with pytest.raises(RuntimeError, match=r"self-contained.*libssl\.so\.3.*auditwheel"):
        native_wheel.validate_system_dependencies(artifact, "x86_64-unknown-linux-gnu")


def test_macos_dependency_audit_parses_and_rejects_external_rpaths(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    artifact = tmp_path / "libdemo.dylib"
    artifact.write_bytes(b"native")
    output = (
        f"{artifact}:\n"
        "\t@rpath/libdemo.dylib (compatibility version 0.0.0, current version 0.0.0)\n"
        "\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)\n"
    )
    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(return_value=completed(output)))
    assert native_wheel.read_macos_dependencies(artifact) == {
        "@rpath/libdemo.dylib",
        "/usr/lib/libSystem.B.dylib",
    }
    native_wheel.validate_system_dependencies(artifact, "aarch64-apple-darwin")

    external = output + "\t@rpath/libcrypto.3.dylib (compatibility version 3.0.0, current version 3.0.0)\n"
    monkeypatch.setattr(native_wheel.subprocess, "run", unittest.mock.Mock(return_value=completed(external)))
    with pytest.raises(RuntimeError, match="libcrypto"):
        native_wheel.validate_system_dependencies(artifact, "aarch64-apple-darwin")


def test_windows_dependency_audit_uses_system_resolution(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    artifact = tmp_path / "demo.dll"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(
        native_wheel,
        "read_windows_dependencies",
        lambda _: {"kernel32.dll", "api-ms-win-core-file-l1-1-0.dll"},
    )
    monkeypatch.setattr(
        native_wheel,
        "is_windows_system_dependency",
        lambda name: name.startswith("api-ms-win-") or name == "kernel32.dll",
    )
    native_wheel.validate_system_dependencies(artifact, "x86_64-pc-windows-msvc")

    monkeypatch.setattr(native_wheel, "read_windows_dependencies", lambda _: {"libcrypto-3-x64.dll"})
    with pytest.raises(RuntimeError, match="libcrypto"):
        native_wheel.validate_system_dependencies(artifact, "x86_64-pc-windows-msvc")


def test_stage_cleanup_is_narrow(tmp_path: pathlib.Path) -> None:
    project = tmp_path / "project"
    stage = native_wheel.wheel_stage_dir(project, pathlib.Path("dist"))
    stage.mkdir(parents=True)
    (stage / "libdemo.so").write_bytes(b"native")
    keep = project / "dist/keep.txt"
    keep.write_text("keep")
    native_wheel.cleanup_wheel_stage(project, pathlib.Path("dist"))
    assert not stage.exists()
    assert keep.read_text() == "keep"
