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
HARDWARE_LOCK = ROOT / "hardware" / "rspyts.lock"
EVALUATION_LOCK = ROOT / "evaluation" / "rspyts.lock"
EVALUATION_CRATE = ROOT / "evaluation" / "rust"


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
path = {json.dumps(str(EVALUATION_CRATE))}

[python]
package = "neurovirtual.evaluation"

[typescript]
package = "@neurovirtual/evaluation"
mode = "static"

[dependencies.hardware]
crate = "cross-package-hardware"
lock = "hardware.lock"
python = "neurovirtual.hardware"
typescript = "@neurovirtual/hardware"
'''
    )
    return config


with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    hardware = json.loads(HARDWARE_LOCK.read_text())
    hardware["fingerprint"] = "sha256:" + "0" * 64
    (directory / "hardware.lock").write_text(
        json.dumps(hardware, separators=(",", ":")) + "\n"
    )
    stale = run("inspect", "--config", str(config))
    assert stale.returncode != 0, "a stale Hardware lock was accepted"
    assert "lock fingerprint mismatch" in stale.stderr, stale.stderr

with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    (directory / "hardware.lock").write_bytes(HARDWARE_LOCK.read_bytes())
    evaluation = json.loads(EVALUATION_LOCK.read_text())
    evaluation["dependencies"]["hardware"]["fingerprint"] = (
        "sha256:" + "f" * 64
    )
    (directory / "rspyts.lock").write_text(
        json.dumps(evaluation, separators=(",", ":")) + "\n"
    )
    mismatch = run("check", "--locked", "--config", str(config))
    assert mismatch.returncode != 0, "a mismatched Evaluation lock was accepted"
    assert "contract lock fingerprint mismatch" in mismatch.stderr, mismatch.stderr
