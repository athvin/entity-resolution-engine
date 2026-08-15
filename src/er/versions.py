"""The S2.1 pin table in Python, and the installed-environment parity check.

S2.1 is the authority for every version this project pins; this module is the one
Python restatement of it. `er doctor` (S4.0, T-DOCTOR-1) iterates :data:`PINS`, the
Dockerfile extension check reads :data:`EXTENSION_PINS`, and the Compose contract
test reads :data:`IMAGE_PINS` — so a bump lands in one table rather than in three
copies of a version string that drift apart silently.

The module is pure: it opens no connection, runs no subprocess and touches nothing
but the ``importlib.metadata`` database. Everything that needs a *running* engine —
`duckdb_extensions()`, the catalog round-trip, the S3 round-trip — belongs to
`er doctor` (ER-022), not here.

**Keying.** A component name is the PEP 503 canonical form of what S2.1 writes:
lowercased with runs of ``-_.`` and whitespace collapsed to ``-``. A row that pins
distributions contributes one entry *per distribution*, because that is the
granularity `importlib.metadata` can be asked about — S2.1's ``pytest`` row pins
``pytest`` and ``pytest-xdist``, and its ``typer`` row pins three. Every other row
(the interpreter, the three extensions, the three images) contributes one entry
named after the row itself. ``tests/unit/test_versions.py`` re-derives that set by
parsing S2.1 and asserts equality in both directions, so an added spec row with no
entry here fails on the unit layer.

:data:`PINS` carries **every** S2.1 row, not only the ones `er doctor` asserts; the
``asserted_by_doctor`` flag is what selects the doctor's subset. The rows the
doctor does not assert (``dbt-adapters``/``dbt-common``, ``hypothesis``, the
object-store init image) are still pins that CI and the lockfile enforce, and
leaving them out would let the parity test above pass while the table was
incomplete.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, version
from typing import Final

from er import __version__

__all__ = [
    "CODE_DISTRIBUTION",
    "EXTENSION_PINS",
    "IMAGE_PINS",
    "PINS",
    "SPLINK_MIGRATION_NOTE",
    "ExtensionPin",
    "ImagePin",
    "MigrationNote",
    "Pin",
    "VersionCheck",
    "check_installed_versions",
    "code_version",
    "installed_version",
]


@dataclass(frozen=True, slots=True)
class Pin:
    """One pinned component of S2.1.

    ``version`` is the literal S2.1 states, whatever kind of literal that is: a
    distribution version, the interpreter's ``major.minor``, an extension commit
    hash or a fully qualified image reference. ``distribution`` is set only when the
    pin is a Python distribution this project declares, because that is the only
    case ``importlib.metadata`` can answer — ``uv`` is pinned and asserted by
    `er doctor`, but through ``uv --version``, so it carries no distribution.
    """

    component: str
    version: str
    distribution: str | None
    asserted_by_doctor: bool


@dataclass(frozen=True, slots=True)
class ExtensionPin:
    """A DuckDB extension pinned to the ``extension_version`` commit hash.

    The two names are the point of the type. ``INSTALL postgres`` uses the install
    name, but ``duckdb_extensions()`` reports that extension as ``postgres_scanner``
    — so a check written against the install name matches no row and passes while
    asserting nothing. Consumers MUST filter on :attr:`registered_name` (S2.1).
    """

    install_name: str
    version: str
    registered_name: str


@dataclass(frozen=True, slots=True)
class ImagePin:
    """A Compose service image pinned by digest, never by a mutable tag (S2.1).

    The tag is retained beside the digest because it is what a human reads in
    `docker/compose.yaml`, but :attr:`reference` — tag *and* digest — is what the
    file MUST contain: the digest alone is what makes the pin immutable.
    """

    service: str
    repository: str
    tag: str
    digest: str

    @property
    def reference(self) -> str:
        """The full ``repo:tag@sha256:…`` reference, as S2.1 and Compose write it."""
        return f"{self.repository}:{self.tag}@{self.digest}"


@dataclass(frozen=True, slots=True)
class VersionCheck:
    """One row of :func:`check_installed_versions`: expected, actual, verdict.

    ``actual`` is ``None`` when the distribution is not installed at all, which is a
    failed check rather than an error — the caller is `er doctor`, whose contract is
    to print every check and exit 1, not to abort on the first missing package.
    """

    component: str
    distribution: str
    expected: str
    actual: str | None
    ok: bool


@dataclass(frozen=True, slots=True)
class MigrationNote:
    """A version bump that is a deliberate migration, not a lockfile refresh."""

    distribution: str
    pinned_major: int
    breaking_major: int
    removed: tuple[str, ...]
    replacement: tuple[str, ...]
    blast_radius: tuple[str, ...]
    acceptance_tests: tuple[str, ...]
    rationale: str


#: The distribution this repository builds; :func:`code_version` reads it and every
#: `runs.code_version` value is what that returns.
CODE_DISTRIBUTION: Final[str] = "er"

#: The three DuckDB extensions, keyed by install name. The values are
#: ``extension_version`` commit hashes against the pinned engine build, not
#: semvers: an extension binary is built per engine version, and a mismatch fails
#: at ATTACH rather than at query time (S2.1).
EXTENSION_PINS: Final[Mapping[str, ExtensionPin]] = {
    "ducklake": ExtensionPin("ducklake", "d8a1881e", "ducklake"),
    # The registered name differs from the install name for this one extension, and
    # that asymmetry is the whole reason `registered_name` exists.
    "postgres": ExtensionPin("postgres", "41223e5", "postgres_scanner"),
    "httpfs": ExtensionPin("httpfs", "827222f", "httpfs"),
}

#: The three Compose service images, keyed by the service name `docker/compose.yaml`
#: declares (S7.1), because the consumer is the compose contract test. The catalog
#: digest is the multi-arch index digest, so it resolves on amd64 CI and on arm64
#: developer machines alike. Both minio images are EOL and are pinned by digest
#: precisely so an unmaintained substrate cannot drift (S2.1, S13).
IMAGE_PINS: Final[Mapping[str, ImagePin]] = {
    "catalog": ImagePin(
        "catalog",
        "postgres",
        "16",
        "sha256:11a9d238fbb48bab14599c57e41123254452b1a2d93c6c8595bce96f346bd082",
    ),
    "objectstore": ImagePin(
        "objectstore",
        "minio/minio",
        "RELEASE.2025-09-07T16-13-09Z",
        "sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e",
    ),
    "objectstore-init": ImagePin(
        "objectstore-init",
        "minio/mc",
        "RELEASE.2025-08-13T08-35-41Z",
        "sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727",
    ),
}

# Authored as a sequence so no key can drift from the `component` it is filed
# under; `PINS` below is the mapping every consumer reads. Order is S2.1's row
# order, which is what `er doctor` prints.
_ROWS: Final[tuple[Pin, ...]] = (
    # No distribution backs the interpreter: `er doctor` compares
    # `sys.version_info[:2]` against this literal (S2.1, T-DOCTOR-1).
    Pin("python", "3.12", None, asserted_by_doctor=True),
    Pin("splink", "4.0.16", "splink", asserted_by_doctor=True),
    Pin("duckdb", "1.5.5", "duckdb", asserted_by_doctor=True),
    Pin("dbt-core", "1.12.2", "dbt-core", asserted_by_doctor=True),
    Pin("dbt-duckdb", "1.11.0", "dbt-duckdb", asserted_by_doctor=True),
    # Transitive through dbt-core, so the lockfile is their only assertion — but
    # S5.0 relies on their contract-enforcement machinery, so they are pinned.
    Pin("dbt-adapters", "1.24.5", "dbt-adapters", asserted_by_doctor=False),
    Pin("dbt-common", "1.39.0", "dbt-common", asserted_by_doctor=False),
    Pin("ducklake", EXTENSION_PINS["ducklake"].version, None, asserted_by_doctor=True),
    Pin("postgres", EXTENSION_PINS["postgres"].version, None, asserted_by_doctor=True),
    Pin("httpfs", EXTENSION_PINS["httpfs"].version, None, asserted_by_doctor=True),
    # `uv` produced the committed lockfile every other pin derives from, but it is
    # not installed into this environment; `er doctor` reads `uv --version`.
    Pin("uv", "0.11.3", None, asserted_by_doctor=True),
    Pin("ruff", "0.16.3", "ruff", asserted_by_doctor=True),
    Pin("mypy", "2.3.0", "mypy", asserted_by_doctor=True),
    Pin("pytest", "9.1.1", "pytest", asserted_by_doctor=True),
    Pin("pytest-xdist", "3.8.0", "pytest-xdist", asserted_by_doctor=True),
    Pin("hypothesis", "6.165.7", "hypothesis", asserted_by_doctor=False),
    # The wheel ships the `actionlint` binary, which is why nothing downloads it.
    Pin("actionlint-py", "1.7.7.23", "actionlint-py", asserted_by_doctor=True),
    Pin("typer", "0.27.1", "typer", asserted_by_doctor=True),
    Pin("pydantic", "2.13.4", "pydantic", asserted_by_doctor=True),
    Pin("python-ulid", "4.0.1", "python-ulid", asserted_by_doctor=True),
    Pin("boto3", "1.43.72", "boto3", asserted_by_doctor=True),
    # S2.1 pins `psycopg[binary]`; the extra selects a wheel, it is not part of the
    # distribution name `importlib.metadata` knows.
    Pin("psycopg", "3.3.4", "psycopg", asserted_by_doctor=True),
    Pin("pyyaml", "6.0.3", "pyyaml", asserted_by_doctor=True),
    Pin("types-pyyaml", "6.0.12.20260724", "types-pyyaml", asserted_by_doctor=True),
    Pin("catalog-image", IMAGE_PINS["catalog"].reference, None, asserted_by_doctor=True),
    Pin("object-store-image", IMAGE_PINS["objectstore"].reference, None, asserted_by_doctor=True),
    Pin(
        "object-store-init-image",
        IMAGE_PINS["objectstore-init"].reference,
        None,
        asserted_by_doctor=False,
    ),
)

#: Every S2.1 row, keyed by component name. `er doctor` iterates the
#: ``asserted_by_doctor`` subset; the static job cross-checks the whole table
#: against `uv.lock` and `docker/compose.yaml`.
PINS: Final[Mapping[str, Pin]] = {pin.component: pin for pin in _ROWS}

#: The S13 Splink 5 row, machine-readable. The splink pin's major is asserted
#: against :attr:`MigrationNote.pinned_major` in the unit layer, so bumping the pin
#: to 5 turns a test red rather than quietly removing the primitive the D2
#: new-vs-corpus pass is built on.
SPLINK_MIGRATION_NOTE: Final[MigrationNote] = MigrationNote(
    distribution="splink",
    pinned_major=4,
    breaking_major=5,
    removed=(
        "find_matches_to_new_records",
        "use_cache",
        "materialise_blocked_pairs",
    ),
    replacement=("predict_between", "predict_within"),
    blast_radius=("src/er/matching/incremental.py",),
    acceptance_tests=("T-INC-3", "T-BLK-1"),
    rationale=(
        "Splink 5 removes find_matches_to_new_records, the exact primitive D2's "
        "new-vs-corpus pass depends on, along with implicit caching, use_cache, "
        "materialise_blocked_pairs and salting; score_pairs is cartesian and is not a "
        "substitute. Blast radius is one module, src/er/matching/incremental.py, "
        "because nothing outside it calls Splink inference. Migration replaces the "
        "two-pass union with predict_between + predict_within; T-INC-3 and T-BLK-1 "
        "are the acceptance gate and MUST be green on the current pin first (S13)."
    ),
)


def installed_version(distribution: str) -> str | None:
    """The installed version of ``distribution``, or ``None`` if it is absent.

    Absence is data, not an exception: `er doctor` prints one line per check and
    exits 1, so a missing distribution must reach it as a failed row rather than as
    a traceback that hides the twenty checks after it.
    """
    try:
        return version(distribution)
    except PackageNotFoundError:
        return None


def check_installed_versions(pins: Mapping[str, Pin] = PINS) -> tuple[VersionCheck, ...]:
    """Compare every distribution-backed pin against the installed environment.

    One row per pin that names a distribution, in ``pins`` order. The rows that name
    none — the interpreter, the extensions, the images — are not skipped silently
    but out of scope: nothing in the metadata database can answer for them, and
    `er doctor` asserts each against its own runtime source.
    """
    checks: list[VersionCheck] = []
    for pin in pins.values():
        if pin.distribution is None:
            continue
        actual = installed_version(pin.distribution)
        checks.append(
            VersionCheck(
                component=pin.component,
                distribution=pin.distribution,
                expected=pin.version,
                actual=actual,
                ok=actual == pin.version,
            )
        )
    return tuple(checks)


def code_version() -> str:
    """The version of this package — the value every ``runs.code_version`` carries.

    Prefers the metadata of the installed distribution, which is what an editable
    install and a wheel both provide. The fallback to the package attribute keeps
    this total for a bare source checkout: a run recording an approximate
    ``code_version`` is better than a run that cannot start.
    """
    installed = installed_version(CODE_DISTRIBUTION)
    return installed if installed is not None else __version__
