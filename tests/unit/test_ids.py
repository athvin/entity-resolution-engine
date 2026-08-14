"""Unit tests for the identity primitives of S5.0/S4.5.3 (S8.4, last bullet).

Each test names the property the spec states rather than the implementation that
happens to satisfy it: the ``':'`` ban and its round trip (D6), orientation
invariance of pair canonicalisation (D9), a strictly ascending id stream even
when the clock cannot move (MINOR-``event_id``), a reproducible test factory
(D10), the redirect cycle guard (S4.5.3), and the isolation that lets every other
package import this module (S5.0).
"""

from __future__ import annotations

import ast
import json
import os
import random
import subprocess
import sys
from itertools import pairwise
from pathlib import Path

import pytest

from er.entities.ids import (
    RECORD_KEY_SEPARATOR,
    CountingIdFactory,
    IdCycleError,
    InvalidRecordKeyError,
    MonotonicUlidFactory,
    UlidFactory,
    canonicalize_pair,
    record_key,
    resolve,
    split_record_key,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
IDS_SOURCE = REPO_ROOT / "src" / "er" / "entities" / "ids.py"

ULID_LENGTH = 26

# Round-trip fixtures. `|` is here because it is the natural delimiter someone
# reaches for when the `':'` one is banned, and the non-ASCII ids because
# `record_key` feeds Splink's unique_id_column_name and must not be ASCII-only.
ROUND_TRIP_FIXTURES = [
    ("crm", "123"),
    ("billing", "acct|0001"),
    ("billing", "a|b|c"),
    ("webforms", "Zoë-Ω-42"),
    ("webforms", "顧客-7"),
    ("crm", "id with spaces"),
]

# One millisecond the clock never leaves, so monotonicity cannot come from time.
FROZEN_MILLIS = 1_700_000_000_000


def run_in_subprocess(script: str) -> str:
    """Run `script` in a fresh interpreter against this working tree's `src/`."""
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO_ROOT / "src")
    result = subprocess.run(
        [sys.executable, "-c", script],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout


def test_record_key_rejects_colon() -> None:
    assert record_key("crm", "123") == "crm:123"
    assert RECORD_KEY_SEPARATOR == ":"

    with pytest.raises(InvalidRecordKeyError) as banned:
        record_key("crm", "1:2")
    assert "':'" in str(banned.value)

    # split_record_key splits on the FIRST separator, so a colon in the system
    # name would break the round trip just as surely.
    with pytest.raises(InvalidRecordKeyError):
        record_key("crm:eu", "123")

    with pytest.raises(InvalidRecordKeyError):
        record_key("", "123")
    with pytest.raises(InvalidRecordKeyError):
        record_key("crm", "")


def test_record_key_round_trips_through_split() -> None:
    for source_system, source_record_id in ROUND_TRIP_FIXTURES:
        key = record_key(source_system, source_record_id)
        assert key == f"{source_system}:{source_record_id}"
        assert split_record_key(key) == (source_system, source_record_id)

    for malformed in ("no-separator", ":123", "crm:"):
        with pytest.raises(InvalidRecordKeyError):
            split_record_key(malformed)


def test_canonicalize_pair_is_orientation_invariant() -> None:
    rng = random.Random(20260814)  # seeded: a failing pair must be reproducible
    systems = ("crm", "billing", "webforms")
    for _ in range(1000):
        a = record_key(rng.choice(systems), str(rng.randrange(10_000)))
        b = record_key(rng.choice(systems), str(rng.randrange(10_000)))
        if a == b:
            continue
        canonical = canonicalize_pair(a, b)
        assert canonical == canonicalize_pair(b, a)
        assert canonical == (min(a, b), max(a, b))
        assert canonical[0] < canonical[1]


def test_canonicalize_pair_rejects_self_pair() -> None:
    with pytest.raises(ValueError):
        canonicalize_pair("crm:1", "crm:1")


def test_monotonic_ulid_is_strictly_increasing_within_one_millisecond() -> None:
    # The AC's own expression: a fresh factory per id. Monotonicity is a property
    # of the process's id stream, so it must survive that.
    ids = [MonotonicUlidFactory().new() for _ in range(10_000)]
    assert all(len(i) == ULID_LENGTH for i in ids)
    assert all(a < b for a, b in pairwise(ids))

    frozen = MonotonicUlidFactory(clock=lambda: FROZEN_MILLIS)
    frozen_ids = [frozen.new() for _ in range(10_000)]
    assert all(len(i) == ULID_LENGTH for i in frozen_ids)
    assert len(set(frozen_ids)) == len(frozen_ids)
    assert all(a < b for a, b in pairwise(frozen_ids))
    # All 10k really did land in the frozen millisecond, or the assertion above
    # would have been proved by the clock rather than by the policy.
    assert len({i[:10] for i in frozen_ids}) == 1

    # A clock that steps backwards must stall, never emit a smaller id.
    readings = iter([FROZEN_MILLIS, FROZEN_MILLIS - 5_000, FROZEN_MILLIS + 1])
    regressing = MonotonicUlidFactory(clock=lambda: next(readings))
    stepped = [regressing.new() for _ in range(3)]
    assert stepped == sorted(stepped)
    assert len(set(stepped)) == 3

    assert len(UlidFactory().new()) == ULID_LENGTH


def test_counting_factory_is_reproducible_across_processes() -> None:
    script = (
        "import json;"
        "from er.entities.ids import CountingIdFactory;"
        "f = CountingIdFactory();"
        "print(json.dumps([f.new() for _ in range(64)]))"
    )
    first = json.loads(run_in_subprocess(script))
    second = json.loads(run_in_subprocess(script))
    factory = CountingIdFactory()
    in_process = [factory.new() for _ in range(64)]

    assert first == second
    assert first == in_process
    assert all(len(i) == ULID_LENGTH for i in first)
    assert first == sorted(first)
    assert len(set(first)) == len(first)


def test_resolve_follows_three_deep_chain() -> None:
    assert resolve("a", {"a": "b", "b": "c"}) == "c"
    assert resolve("a", {"a": "b", "b": "c", "c": "d"}) == "d"
    assert resolve("z", {}) == "z"
    # An id that is only ever a claimant resolves to itself.
    assert resolve("d", {"a": "b", "b": "c", "c": "d"}) == "d"


def test_resolve_raises_on_cycle() -> None:
    with pytest.raises(IdCycleError) as two_cycle:
        resolve("a", {"a": "b", "b": "a"})
    assert "a -> b -> a" in str(two_cycle.value)

    with pytest.raises(IdCycleError):
        resolve("a", {"a": "a"})

    # A chain longer than max_hops must also stop, rather than walk it. max_hops
    # counts redirects followed, so a 100-redirect chain resolves at exactly 100.
    chain = {str(i): str(i + 1) for i in range(100)}
    with pytest.raises(IdCycleError) as too_long:
        resolve("0", chain, max_hops=8)
    assert "max_hops=8" in str(too_long.value)
    with pytest.raises(IdCycleError):
        resolve("0", chain, max_hops=99)
    assert resolve("0", chain, max_hops=100) == "100"


def test_ids_module_has_no_intra_package_imports() -> None:
    tree = ast.parse(IDS_SOURCE.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                assert not alias.name.startswith("er."), f"ids.py imports {alias.name}"
        elif isinstance(node, ast.ImportFrom):
            assert node.level == 0, "ids.py uses a relative import"
            module = node.module or ""
            assert module != "er" and not module.startswith("er."), f"ids.py imports from {module}"

    # The transitive graph, not just the direct imports: importing the module in a
    # fresh interpreter may pull in its parent packages and nothing else.
    loaded = json.loads(
        run_in_subprocess(
            "import json, sys;"
            "import er.entities.ids;"
            "print(json.dumps(sorted(m for m in sys.modules "
            "if m == 'er' or m.startswith('er.'))))"
        )
    )
    assert loaded == ["er", "er.entities", "er.entities.ids"]
