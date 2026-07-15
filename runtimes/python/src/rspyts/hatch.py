"""Hatch build-hook adapter for rspyts-backed Python packages."""

from __future__ import annotations

import pathlib
from importlib.metadata import version as distribution_version
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface
from hatchling.plugin import hookimpl

from .native_wheel import (
    build_native,
    cleanup_wheel_stage,
    configure_standard_wheel,
    resolve_hook_config,
)


class RspytsBuildHook(BuildHookInterface):
    """Build and bundle the host cdylib declared by ``rspyts.toml``."""

    PLUGIN_NAME = "rspyts"

    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        """Prepare a standard wheel or stage a development editable library."""
        if self.target_name == "sdist":
            raise RuntimeError(
                "rspyts-backed packages are wheel-only; source distributions cannot contain the native library"
            )
        if self.target_name != "wheel" or version not in {"editable", "standard"}:
            return

        project_root = pathlib.Path(self.root).resolve()
        config_path = resolve_hook_config(project_root, self.config)
        build = build_native(
            project_root=project_root,
            config_path=config_path,
            build_directory=pathlib.Path(self.directory),
            runtime_version=distribution_version("rspyts"),
            editable=version == "editable",
        )
        if version == "standard":
            try:
                configure_standard_wheel(build_data, build, project_root)
            except Exception:
                cleanup_wheel_stage(project_root, pathlib.Path(self.directory))
                raise

    def finalize(self, version: str, build_data: dict[str, Any], artifact_path: str) -> None:
        """Remove the standard wheel's temporary native staging directory."""
        del build_data, artifact_path
        if self.target_name == "wheel" and version == "standard":
            cleanup_wheel_stage(pathlib.Path(self.root).resolve(), pathlib.Path(self.directory))


@hookimpl
def hatch_register_build_hook() -> type[RspytsBuildHook]:
    """Register the rspyts build hook with Hatch."""
    return RspytsBuildHook
