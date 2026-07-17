"""Prove both installed-style packages share one Python class object."""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).parent
with tempfile.TemporaryDirectory() as directory:
    site_packages = Path(directory)
    namespace = site_packages / "neurovirtual"
    shutil.copytree(
        ROOT / "hardware" / ".rspyts" / "python" / "neurovirtual",
        namespace,
    )
    shutil.copytree(
        ROOT
        / "evaluation"
        / ".rspyts"
        / "python"
        / "neurovirtual"
        / "evaluation",
        namespace / "evaluation",
    )
    sys.path.insert(0, str(site_packages))

    from neurovirtual.evaluation import (  # noqa: PLC0415
        SignalDefinition as EvaluationSignalDefinition,
        evaluate,
    )
    from neurovirtual.hardware import (  # noqa: PLC0415
        SignalDefinition as HardwareSignalDefinition,
    )

    assert EvaluationSignalDefinition is HardwareSignalDefinition
    signal = HardwareSignalDefinition(id="sig:c3", sample_rate_hz=256)
    result = evaluate(signal, 1)
    assert result.signal is signal or result.signal == signal
    assert isinstance(result.signal, HardwareSignalDefinition)
