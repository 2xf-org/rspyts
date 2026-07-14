#!/usr/bin/env python3
"""Fail a release when its tag and package versions are not in lockstep."""

from __future__ import annotations

import json
import os
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read_toml(path: Path) -> dict:
    with path.open("rb") as file:
        return tomllib.load(file)


def fail(message: str) -> None:
    print(f"release validation failed: {message}", file=sys.stderr)
    raise SystemExit(1)


cargo = read_toml(ROOT / "Cargo.toml")
version = cargo["workspace"]["package"]["version"]

if not re.fullmatch(r"\d+\.\d+\.\d+", version):
    fail(f"workspace version {version!r} is not a stable SemVer version")

tag = os.environ.get("GITHUB_REF_NAME", f"v{version}")
if tag != f"v{version}":
    fail(f"tag {tag!r} must exactly match workspace version {version!r}")

for dependency in ("rspyts-core", "rspyts-macros", "rspyts"):
    actual = cargo["workspace"]["dependencies"][dependency]["version"]
    if actual != version:
        fail(f"{dependency} dependency pin is {actual!r}, expected {version!r}")

python_project = read_toml(ROOT / "runtimes/python/pyproject.toml")
if python_project["project"]["version"] != version:
    fail("Python project version does not match the workspace")

python_init = (ROOT / "runtimes/python/src/rspyts/__init__.py").read_text()
if f'__version__ = "{version}"' not in python_init:
    fail("rspyts.__version__ does not match the workspace")

python_lock = read_toml(ROOT / "runtimes/python/uv.lock")
locked_python = [
    package["version"]
    for package in python_lock["package"]
    if package["name"] == "rspyts"
]
if locked_python != [version]:
    fail(f"Python lock version is {locked_python!r}, expected [{version!r}]")

typescript = json.loads((ROOT / "runtimes/typescript/package.json").read_text())
typescript_lock = json.loads(
    (ROOT / "runtimes/typescript/package-lock.json").read_text()
)
if typescript["version"] != version:
    fail("npm package version does not match the workspace")
if typescript_lock["version"] != version:
    fail("npm lockfile version does not match the workspace")
if typescript_lock["packages"][""]["version"] != version:
    fail("npm root lockfile package version does not match the workspace")

print(f"release versions are synchronized at {version} ({tag})")
