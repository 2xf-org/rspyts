"""Assert generated source preserves the defining package's model identity."""

from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).parent
CATALOG = ROOT / "catalog"
REPORTS = ROOT / "reports"

catalog_lock = json.loads((CATALOG / "rspyts.lock").read_text())
reports_lock = json.loads((REPORTS / "rspyts.lock").read_text())
models = (
    REPORTS
    / ".rspyts"
    / "python"
    / "example"
    / "reports"
    / "models.py"
).read_text()
functions = (
    REPORTS
    / ".rspyts"
    / "python"
    / "example"
    / "reports"
    / "functions.py"
).read_text()
typescript_declarations = (
    REPORTS / ".rspyts" / "typescript" / "index.d.ts"
).read_text()
typescript_runtime = (
    REPORTS / ".rspyts" / "typescript" / "index.js"
).read_text()
reports_contract = json.loads(
    (REPORTS / ".rspyts" / "contract.json").read_text()
)

assert "class SignalDefinition" not in models, (
    "Reports generated a second SignalDefinition instead of importing "
    "example.catalog.SignalDefinition"
)
assert "from example.catalog import SignalDefinition" in models, (
    "Reports does not import Catalog's model identity"
)
assert "def define_signal" not in functions, (
    "Reports re-exported a function owned by Catalog"
)
assert (
    'import type { SignalDefinition } from "@example/catalog";'
    in typescript_declarations
), "Reports TypeScript does not import Catalog's type identity"
assert "interface SignalDefinition" not in typescript_declarations, (
    "Reports emitted a duplicate TypeScript SignalDefinition"
)
assert "defineSignal" not in typescript_declarations
assert "defineSignal" not in typescript_runtime
assert all(
    function["owner"] == "cross-package-reports"
    for function in reports_contract["manifest"]["functions"]
), "Reports's contract absorbed a Catalog-owned export"
assert reports_lock["dependencies"]["catalog"]["fingerprint"] == catalog_lock[
    "fingerprint"
], "Reports's lock does not pin Catalog's semantic fingerprint"
assert reports_lock["dependencies"]["catalog"]["crate"] == (
    "cross-package-catalog"
)
assert reports_lock["dependencies"]["catalog"]["python"] == (
    "example.catalog"
)
assert reports_lock["dependencies"]["catalog"]["typescript"] == (
    "@example/catalog"
)
