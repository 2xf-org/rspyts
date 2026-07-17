"""Prove stale dependency and consumer locks fail closed."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parents[1]
RSPYTS = Path(os.environ.get("RSPYTS_BIN", REPOSITORY / "target/debug/rspyts"))
OWNER_LOCK = ROOT / "owner" / "rspyts.lock"
CONSUMER_LOCK = ROOT / "consumer" / "rspyts.lock"
CONSUMER_CRATE = ROOT / "consumer" / "rust"
OWNER_CRATE = ROOT / "owner" / "rust"


def run(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(RSPYTS), *arguments],
        check=False,
        capture_output=True,
        text=True,
    )


def copy_crate(directory: Path, source: Path) -> None:
    shutil.copytree(source, directory / "rust")
    with (directory / "rust" / "Cargo.toml").open("a") as manifest:
        manifest.write("\n[workspace]\n")
    subprocess.run(
        [
            "cargo",
            "generate-lockfile",
            "--offline",
            "--manifest-path",
            str(directory / "rust" / "Cargo.toml"),
        ],
        check=True,
        capture_output=True,
        text=True,
    )


def write_config(directory: Path) -> Path:
    copy_crate(directory, CONSUMER_CRATE)
    config = directory / "rspyts.toml"
    config.write_text(
        '''[crate]
path = "rust"

[python]
package = "example.consumer.contracts"

[typescript]
package = "@example/consumer"
mode = "static"

[dependencies.owner]
crate = "cross-package-owner"
lock = "owner.lock"
python = "example.owner.contracts"
typescript = "@example/owner"
'''
    )
    return config


def write_owner_config(directory: Path) -> Path:
    copy_crate(directory, OWNER_CRATE)
    config = directory / "rspyts.toml"
    config.write_text(
        '''[crate]
path = "rust"

[python]
package = "example.owner.contracts"

[typescript]
package = "@example/owner"
mode = "wasm"
'''
    )
    return config


with tempfile.TemporaryDirectory(dir=ROOT) as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    owner = json.loads(OWNER_LOCK.read_text())
    owner["fingerprint"] = "sha256:" + "0" * 64
    (directory / "owner.lock").write_text(
        json.dumps(owner, separators=(",", ":")) + "\n"
    )
    stale = run("inspect", "--config", str(config))
    assert stale.returncode != 0, "a stale owner lock was accepted"
    assert "lock fingerprint mismatch" in stale.stderr, stale.stderr

with tempfile.TemporaryDirectory(dir=ROOT) as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    (directory / "owner.lock").write_bytes(OWNER_LOCK.read_bytes())
    consumer = json.loads(CONSUMER_LOCK.read_text())
    consumer["dependencies"]["owner"]["fingerprint"] = "sha256:" + "f" * 64
    (directory / "rspyts.lock").write_text(
        json.dumps(consumer, separators=(",", ":")) + "\n"
    )
    mismatch = run("check", "--locked", "--config", str(config))
    assert mismatch.returncode != 0, "a mismatched consumer lock was accepted"
    assert "contract lock fingerprint mismatch" in mismatch.stderr, mismatch.stderr

with tempfile.TemporaryDirectory(dir=ROOT) as temporary:
    directory = Path(temporary)
    config = write_owner_config(directory)
    owner = json.loads(OWNER_LOCK.read_text())
    owner["manifest"]["crateVersion"] = "9.9.9"
    (directory / "rspyts.lock").write_text(
        json.dumps(owner, separators=(",", ":")) + "\n"
    )
    mismatch = run("check", "--locked", "--config", str(config))
    assert mismatch.returncode != 0, "a stale owner crateVersion was accepted"
    assert "crate version `0.1.0` does not match locked version `9.9.9`" in (
        mismatch.stderr
    ), mismatch.stderr

with tempfile.TemporaryDirectory(dir=ROOT) as temporary:
    directory = Path(temporary)
    config = write_config(directory)
    owner = json.loads(OWNER_LOCK.read_text())
    owner["manifest"]["crateVersion"] = "0.1.1"
    (directory / "owner.lock").write_text(
        json.dumps(owner, separators=(",", ":")) + "\n"
    )
    (directory / "rspyts.lock").write_bytes(CONSUMER_LOCK.read_bytes())
    mismatch = run("check", "--locked", "--config", str(config))
    assert mismatch.returncode != 0, "a stale dependency crateVersion was accepted"
    assert (
        "dependency `owner` crate version changed from `0.1.0` to `0.1.1`"
        in mismatch.stderr
    ), mismatch.stderr
    assert "No semantic contract changes" not in mismatch.stderr
