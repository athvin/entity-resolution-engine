"""S8.1 step 1's xdist suffix, as a pure function (DesignDoc.md S8.1).

The rest of the harness contract is only observable against the substrate, but
this half is not: `ns = f"{ulid}_{worker}"` is arithmetic on two strings, and it
is the one part of step 1 a *session* cannot check about itself. One session
mints exactly one identifier under one worker, so "two workers never collide"
has no witness inside it — which is why the derivation is factored out of
`mint_namespace` and tested here, on the bare runner S8.1 gives the unit layer.

Integration tests run single-process; `-n auto` is a unit-layer option only
(S8.1). The suffix therefore exists for this layer and for nothing else, and a
future `-n auto` run that minted one namespace per worker would otherwise have
every worker racing for one catalog schema.

The function arrives as a fixture rather than as an import because three files
in this tree are named `conftest`, pytest imports each under that one module
name, and `import conftest` resolves to whichever it loaded last -- here, the
dbt macro harness.
"""

from __future__ import annotations

from collections.abc import Callable

import pytest

# What the `derive_namespace` fixture hands back: (identifier, worker) -> namespace.
Deriver = Callable[[str, str | None], str]

# A ULID as S8.1 step 1 mints it: 26 Crockford characters, lowercased. Spelled as
# a literal rather than minted, because every claim below is about what the
# derivation does *to* an identifier and none of them is about ULIDs.
ULID = "01k2qzh8v9r7bnfd3ecmxtpwya"

# Two xdist worker names in the shape the plugin exports them.
WORKERS = ("gw0", "gw3")


def test_xdist_worker_suffix_applied(derive_namespace: Deriver) -> None:
    suffixed = derive_namespace(ULID, "gw3")

    assert suffixed.endswith("_gw3")
    assert suffixed == f"{ULID}_gw3"

    plain = derive_namespace(ULID, None)

    assert plain == ULID
    assert not plain.endswith("_gw3")
    # The distinction the suffix exists to make: absent is not "some worker".
    assert plain != suffixed


@pytest.mark.parametrize("blank", ["", " ", "\t"])
def test_blank_worker_is_absent_not_a_worker(derive_namespace: Deriver, blank: str) -> None:
    # xdist exports `PYTEST_XDIST_WORKER` only in a worker process, so a blank
    # value means the variable was set by something else. Treating it as a worker
    # would append a bare underscore and produce a schema name that no
    # `er_test_<ulid>_<worker>` reader could parse back into the pair it came from.
    assert derive_namespace(ULID, blank) == ULID


def test_namespaces_are_unique_per_worker(derive_namespace: Deriver) -> None:
    derived = {worker: derive_namespace(ULID, worker) for worker in WORKERS}

    assert len(set(derived.values())) == len(WORKERS), (
        f"two workers derived the same namespace from one ULID: {derived}"
    )
    for worker, namespace in derived.items():
        assert namespace == f"{ULID}_{worker}"

    # And distinct identifiers stay distinct under one worker, which is the other
    # direction of the same claim: the suffix partitions, it does not collapse.
    other = "01k2qzh8v9r7bnfd3ecmxtpwyb"
    assert derive_namespace(other, "gw0") != derive_namespace(ULID, "gw0")
