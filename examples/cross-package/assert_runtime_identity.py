"""Prove both installed-style packages share one Python class object."""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).parent
with tempfile.TemporaryDirectory() as directory:
    site_packages = Path(directory)
    namespace = site_packages / "example"
    shutil.copytree(
        ROOT / "catalog" / ".rspyts" / "python" / "example",
        namespace,
    )
    shutil.copytree(
        ROOT
        / "reports"
        / ".rspyts"
        / "python"
        / "example"
        / "reports",
        namespace / "reports",
    )
    sys.path.insert(0, str(site_packages))

    from example.reports import (  # noqa: PLC0415
        SignalDefinition as ReportsSignalDefinition,
        evaluate,
    )
    from example.catalog import (  # noqa: PLC0415
        SignalDefinition as CatalogSignalDefinition,
    )

    assert ReportsSignalDefinition is CatalogSignalDefinition
    signal = CatalogSignalDefinition(id="sig:c3", sample_rate_hz=256)
    result = evaluate(signal, 1)
    assert result.signal is signal or result.signal == signal
    assert isinstance(result.signal, CatalogSignalDefinition)
