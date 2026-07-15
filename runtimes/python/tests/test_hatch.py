"""Tests for the thin Hatch-specific plugin adapter."""

from __future__ import annotations

import pathlib
import unittest.mock
from typing import Any, cast

import pytest

import rspyts.hatch as hatch
from rspyts.native_wheel import NativeBuild


def make_hook(tmp_path: pathlib.Path, target: str = "wheel") -> hatch.RspytsBuildHook:
    """Create a hook instance without needing a complete Hatch builder."""
    project = tmp_path / "python"
    project.mkdir(exist_ok=True)
    (tmp_path / "rspyts.toml").write_text('[crate]\npath = "rust"\n')
    return hatch.RspytsBuildHook(
        str(project),
        {"config": "../rspyts.toml"},
        cast(Any, None),
        cast(Any, None),
        "dist",
        target,
    )


def test_plugin_registers_named_build_hook() -> None:
    assert hatch.hatch_register_build_hook() is hatch.RspytsBuildHook
    assert hatch.RspytsBuildHook.PLUGIN_NAME == "rspyts"


def test_standard_wheel_builds_and_configures(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    hook = make_hook(tmp_path)
    project = pathlib.Path(hook.root)
    artifact = project / "dist/.rspyts-native/libdemo.so"
    build = NativeBuild(artifact, "x86_64-unknown-linux-gnu", project / "src/demo/generated", "manylinux_2_28_x86_64")
    build_native = unittest.mock.Mock(return_value=build)
    configure = unittest.mock.Mock()
    monkeypatch.setattr(hatch, "build_native", build_native)
    monkeypatch.setattr(hatch, "configure_standard_wheel", configure)
    build_data: dict[str, object] = {}

    hook.initialize("standard", build_data)

    build_native.assert_called_once_with(
        project_root=project.resolve(),
        config_path=(tmp_path / "rspyts.toml").resolve(),
        build_directory=pathlib.Path("dist"),
        runtime_version="0.3.2",
        editable=False,
    )
    configure.assert_called_once_with(build_data, build, project.resolve())


def test_editable_build_stages_without_wheel_configuration(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    hook = make_hook(tmp_path)
    project = pathlib.Path(hook.root)
    build = NativeBuild(
        project / "src/demo/generated/lib/libdemo.so",
        "x86_64-unknown-linux-gnu",
        project / "src/demo/generated",
        "manylinux_2_28_x86_64",
    )
    build_native = unittest.mock.Mock(return_value=build)
    configure = unittest.mock.Mock()
    monkeypatch.setattr(hatch, "build_native", build_native)
    monkeypatch.setattr(hatch, "configure_standard_wheel", configure)

    hook.initialize("editable", {})

    assert build_native.call_args.kwargs["editable"] is True
    configure.assert_not_called()


def test_standard_configuration_failure_cleans_stage(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    hook = make_hook(tmp_path)
    project = pathlib.Path(hook.root)
    build = NativeBuild(
        project / "dist/.rspyts-native/libdemo.so",
        "x86_64-unknown-linux-gnu",
        project / "src/demo/generated",
        "manylinux_2_28_x86_64",
    )
    cleanup = unittest.mock.Mock()
    monkeypatch.setattr(hatch, "build_native", unittest.mock.Mock(return_value=build))
    monkeypatch.setattr(hatch, "configure_standard_wheel", unittest.mock.Mock(side_effect=RuntimeError("bad data")))
    monkeypatch.setattr(hatch, "cleanup_wheel_stage", cleanup)

    with pytest.raises(RuntimeError, match="bad data"):
        hook.initialize("standard", {})

    cleanup.assert_called_once_with(project.resolve(), pathlib.Path("dist"))


def test_sdist_fails_before_reading_configuration(tmp_path: pathlib.Path) -> None:
    hook = hatch.RspytsBuildHook(
        str(tmp_path),
        {},
        cast(Any, None),
        cast(Any, None),
        "dist",
        "sdist",
    )
    with pytest.raises(RuntimeError, match="wheel-only"):
        hook.initialize("standard", {})


def test_irrelevant_lifecycle_is_ignored_and_finalize_cleans_standard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    hook = make_hook(tmp_path, target="other")
    build = unittest.mock.Mock()
    monkeypatch.setattr(hatch, "build_native", build)
    hook.initialize("standard", {})
    build.assert_not_called()

    wheel = make_hook(tmp_path, target="wheel")
    cleanup = unittest.mock.Mock()
    monkeypatch.setattr(hatch, "cleanup_wheel_stage", cleanup)
    wheel.initialize("unknown", {})
    wheel.finalize("editable", {}, "editable.whl")
    cleanup.assert_not_called()
    wheel.finalize("standard", {}, "package.whl")
    cleanup.assert_called_once()
