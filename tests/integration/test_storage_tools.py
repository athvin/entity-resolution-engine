"""Fresh pipeline images supply the exact storage releases without registry pulls."""

import subprocess
from pathlib import Path

import pytest

from er.versions import SOURCE_PINS


@pytest.mark.parametrize("service", ["objectstore", "objectstore-init"])
def test_bundled_storage_tool_matches_pinned_source(service):
    pin = SOURCE_PINS[service]
    binary = pin.repository.split("/")[-1]
    result = subprocess.run([binary, "--version"], capture_output=True, text=True, check=True)
    assert pin.release in result.stdout
    assert pin.commit in result.stdout
    assert (Path("/usr/share/licenses") / binary / "LICENSE").is_file()
    assert (Path("/usr/share/licenses") / binary / "NOTICE").is_file()
