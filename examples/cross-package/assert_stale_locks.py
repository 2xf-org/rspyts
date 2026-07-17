"""Prove stale dependency and consumer locks fail closed."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parents[1]
RSPYTS = Path(os.environ.get("RSPYTS_BIN", REPOSITORY / "target/debug/rspyts"))
CATALOG_LOCK = ROOT / "catalog" / "rspyts.lock"
REPORTS_LOCK = ROOT / "reports" / "rspyts.lock"
REPORTS_CRATE = ROOT / "reports" / "rust"


def run(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(RSPYTS), *arguments],
        check=False,
        capture_output=True,
        text=True,
    )


def write_config(directory: Path) -> Path:
    config = directory / "rspyts.toml"
    config.write_text(
        f'''[crate]
path = {json.dumps(str(REPORTS_CRATE))}

[python]
package = "example.reports"

[typescript]
package = "@example/reports"
mode = "static"

[dependencies.catalog]
crate = "cross-package-catalog"
lock = "catalog.lock"
python = "example.catalog"
typescript = "@example/catalog"
'''
    )
    return config


with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    catalog = json.loads(CATALOG_LOCK.read_text())
    catalog["fingerprint"] = "sha256:" + "0" * 64
    (directory / "catalog.lock").write_text(
        json.dumps(catalog, separators=(",", ":")) + "\n"
    )
    stale = run("inspect", "--config", str(config))
    assert stale.returncode != 0, "a stale Catalog lock was accepted"
    assert "lock fingerprint mismatch" in stale.stderr, stale.stderr

with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    (directory / "catalog.lock").write_bytes(CATALOG_LOCK.read_bytes())
    reports = json.loads(REPORTS_LOCK.read_text())
    reports["dependencies"]["catalog"]["fingerprint"] = (
        "sha256:" + "f" * 64
    )
    (directory / "rspyts.lock").write_text(
        json.dumps(reports, separators=(",", ":")) + "\n"
    )
    mismatch = run("check", "--locked", "--config", str(config))
    assert mismatch.returncode != 0, "a mismatched Reports lock was accepted"
    assert "contract lock fingerprint mismatch" in mismatch.stderr, mismatch.stderr
