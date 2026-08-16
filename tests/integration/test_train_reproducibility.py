"""T-TRAIN-1 (S8.3): the committed fixture model regenerates byte for byte.

This is the one test that actually trains the artifact every other M3 scenario loads.
It runs the whole path — generate the S10.1 corpus at the S10.2 `10k` scale, ingest it,
standardize it, fit S4.3.2's sequence over it — inside a lake namespace
`scripts/regen_fixture_model.py` mints and reclaims for itself, and then compares what
came out with what is committed.

**Six things MUST be identical for the bytes to match**, and S8.3 names them rather
than leaving them to a test author, because byte equality is a claim about fully
pinned inputs and not about Splink being deterministic: the corpus, the seed, the
scale, the `model_version`, the verbatim `training:` block, and `splink.__version__`.
The comparison below therefore happens in two stages — the pinned inputs first, the
bytes second — so a red run says *which* input moved. "The settings JSON differs" is
true of all six failures and useful for none of them.

It is marked `slow` and excluded from the PR path (M4's exit criterion is a full CI
path under ten minutes, and this run is minutes on its own). That is not a hole: the
artifact it guards is committed, so a change to it is a change to a file in the diff,
and the cheap assertions about that file — the sidecar's SHA-256, the `training:`
block, the TF column set, the bracketed comparison levels — run on every PR from
`tests/unit/fixtures/test_fixture_model.py`. What only this test can prove is that the
committed bytes are what the declared inputs actually produce.

A run writes what it regenerated to `artifacts/fixture_model/` **before** comparing,
which is the bind mount that reaches the host from inside the container (S7.1). A
maintainer whose change to the generator, the config or the pinned Splink was
intentional copies the three files from there over `fixtures/static/`; that is also
how the artifacts are produced in the first place, since nothing else in the tree can
fit a model over ten thousand records.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType
from typing import Final

import pytest
import yaml

#: Every test in this module trains. `-m "not slow"` must collect none of them.
pytestmark = pytest.mark.slow

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: Where a run leaves the artifacts it regenerated, under the one directory a test may
#: write to without dirtying the tree (S7.1) -- the same convention
#: `tests/integration/scenarios/test_base_10_std.py` uses for its computed digests.
EMIT_DIR: Final = REPO_ROOT / "artifacts" / "fixture_model"

CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"
REGEN_SCRIPT: Final = REPO_ROOT / "scripts" / "regen_fixture_model.py"


def regen_module() -> ModuleType:
    """`scripts/regen_fixture_model.py`, loaded by path.

    `scripts/` is not a package and nothing installs it, so the regeneration entry
    point is reached the way any other script would be. Loading it rather than
    re-implementing it is the point: the committed artifacts and this test's
    regeneration must come from ONE code path, or byte equality would be a claim about
    two implementations agreeing rather than about the inputs being pinned.
    """
    spec = importlib.util.spec_from_file_location("regen_fixture_model", REGEN_SCRIPT)
    assert spec is not None and spec.loader is not None, f"cannot load {REGEN_SCRIPT}"
    module = importlib.util.module_from_spec(spec)
    # Registered BEFORE it is executed. The script carries `from __future__ import
    # annotations`, so its `@dataclass` fields are string annotations, and
    # `dataclasses` resolves them through `sys.modules[cls.__module__]` — which is
    # `None` for a module loaded by path and never registered.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_fixture_model_regenerates_byte_for_byte() -> None:
    """The committed artifacts are exactly what the six pinned inputs produce."""
    regen = regen_module()
    result = regen.check(EMIT_DIR)

    # Stage one: the six pinned inputs of S8.3, named individually. A divergence here
    # says the regeneration was a different experiment, and the byte comparison after
    # it could only restate that in hexadecimal.
    assert result.pinned == (), (
        "T-TRAIN-1: a pinned input of the committed model diverged, so the bytes were "
        "never going to match:\n  " + "\n  ".join(result.pinned)
    )

    # Stage two, and only now: the bytes.
    assert result.artifacts == (), (
        f"T-TRAIN-1: the same pinned inputs produced different artifacts: "
        f"{', '.join(result.artifacts)}.\n"
        f"What this run regenerated is at {EMIT_DIR}; copy it over "
        f"{(REPO_ROOT / 'fixtures' / 'static')} only if the change was intended."
    )
    assert result.exit_code == 0, result.report()

    # S8.3 also requires the `training:` block that was persisted alongside the model
    # to match the config verbatim. The sidecar is where this artifact carries it --
    # `model_registry.metrics` is where `er train` carries it, and ER-055's suite
    # asserts that arm against a live registry.
    document = yaml.safe_load(CONFIG_PATH.read_text(encoding="utf-8"))
    assert result.meta["training"] == document["training"]
