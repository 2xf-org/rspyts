"""Prove installed owner and consumer wheels share one Python class object."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from example.consumer import authored_label as consumer_authored_label
from example.consumer.contracts import Item as ConsumerItem
from example.consumer.contracts import select
from example.owner import authored_label as owner_authored_label
from example.owner.contracts import Item as OwnerItem
from example.owner.contracts import ItemKind, Quantity
import example.owner.contracts as owner_contracts


ROOT = Path(__file__).parent

assert owner_authored_label() == "owner-authored"
assert consumer_authored_label() == "consumer-authored"
assert ConsumerItem is OwnerItem

item = OwnerItem(
    id="item-3",
    quantity=Quantity(numerator=12, denominator=1),
    kind=ItemKind.Standard,
    tag=b"\x01\x02\x03\x04",
)
result = select(item, 1)
assert result.item is item or result.item == item
assert isinstance(result.item, OwnerItem)

site_packages = Path(owner_contracts.__file__).resolve().parents[3]
with tempfile.TemporaryDirectory() as directory:
    isolated = Path(directory)
    shutil.copytree(site_packages / "example", isolated / "example")
    owner_fingerprint = json.loads(
        (ROOT / "owner" / "rspyts.lock").read_text()
    )["fingerprint"]
    constants = isolated / "example" / "owner" / "contracts" / "constants.py"
    source = constants.read_text()
    tampered = source.replace(
        json.dumps(owner_fingerprint),
        json.dumps("sha256:" + "0" * 64),
    )
    assert tampered != source
    constants.write_text(tampered)
    mismatch = subprocess.run(
        [sys.executable, "-c", "import example.consumer.contracts"],
        env=os.environ | {"PYTHONPATH": str(isolated)},
        check=False,
        capture_output=True,
        text=True,
    )
    assert mismatch.returncode != 0, "a mismatched installed owner was accepted"
    assert "contract fingerprint mismatch for Python dependency" in mismatch.stderr
