"""Assert generated source preserves the defining package's model identity."""

from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).parent
HARDWARE = ROOT / "hardware"
EVALUATION = ROOT / "evaluation"

hardware_lock = json.loads((HARDWARE / "rspyts.lock").read_text())
evaluation_lock = json.loads((EVALUATION / "rspyts.lock").read_text())
models = (
    EVALUATION
    / ".rspyts"
    / "python"
    / "neurovirtual"
    / "evaluation"
    / "models.py"
).read_text()
functions = (
    EVALUATION
    / ".rspyts"
    / "python"
    / "neurovirtual"
    / "evaluation"
    / "functions.py"
).read_text()
typescript_declarations = (
    EVALUATION / ".rspyts" / "typescript" / "index.d.ts"
).read_text()
typescript_runtime = (
    EVALUATION / ".rspyts" / "typescript" / "index.js"
).read_text()
evaluation_contract = json.loads(
    (EVALUATION / ".rspyts" / "contract.json").read_text()
)

assert "class SignalDefinition" not in models, (
    "Evaluation generated a second SignalDefinition instead of importing "
    "neurovirtual.hardware.SignalDefinition"
)
assert "from neurovirtual.hardware import SignalDefinition" in models, (
    "Evaluation does not import Hardware's model identity"
)
assert "def define_signal" not in functions, (
    "Evaluation re-exported a function owned by Hardware"
)
assert (
    'import type { SignalDefinition } from "@neurovirtual/hardware";'
    in typescript_declarations
), "Evaluation TypeScript does not import Hardware's type identity"
assert "interface SignalDefinition" not in typescript_declarations, (
    "Evaluation emitted a duplicate TypeScript SignalDefinition"
)
assert "defineSignal" not in typescript_declarations
assert "defineSignal" not in typescript_runtime
assert all(
    function["owner"] == "cross-package-evaluation"
    for function in evaluation_contract["manifest"]["functions"]
), "Evaluation's contract absorbed a Hardware-owned export"
assert evaluation_lock["dependencies"]["hardware"]["fingerprint"] == hardware_lock[
    "fingerprint"
], "Evaluation's lock does not pin Hardware's semantic fingerprint"
assert evaluation_lock["dependencies"]["hardware"]["crate"] == (
    "cross-package-hardware"
)
assert evaluation_lock["dependencies"]["hardware"]["python"] == (
    "neurovirtual.hardware"
)
assert evaluation_lock["dependencies"]["hardware"]["typescript"] == (
    "@neurovirtual/hardware"
)
