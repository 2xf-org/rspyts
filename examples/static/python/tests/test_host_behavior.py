from __future__ import annotations

from example.independent import authored_label
from example.independent.contracts import (
    FORMAT_VERSION,
    ReleaseChannel,
    release_channel_label,
)


def test_authored_source_constants_and_python_behavior() -> None:
    assert authored_label() == "independent-authored"
    assert FORMAT_VERSION == 4
    assert release_channel_label(ReleaseChannel.Beta) == "channel:beta"
