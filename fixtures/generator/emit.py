"""Writes the seeded synthetic corpus as source CSVs (DesignDoc.md S10.1, S8.2, S6).

`personas.py` makes the people and `corruptions.py` makes each source system's view
of them; this module lays those views out as the files `er ingest` reads. Four
constraints shape it, and three of them are constraints the emitted bytes have to
satisfy rather than choices:

* **The headers are S8.2's `base_10` headers, and S6 owns them.** :data:`SOURCE_HEADERS`
  is the literal triple, and :func:`emit_corpus` refuses to write unless it equals
  the header :func:`source_header` derives from `sources.<name>.columns`. That is
  M14 stated twice on purpose: the same `stg_*` models and the same `er ingest` path
  consume the hand-authored fixture and the generated corpus, so a header the
  generator invented would make the generated corpus unrunnable against the models
  the fixture proves -- and it would do so at `er standardize`, thousands of rows in.
* **`persona_id` is the last field of every row** (S8.2, M21), and `truth.csv`
  repeats the same labels as `(persona_id, source_system, source_record_id)` triples.
  The column travels with the row so a benchmark reports match quality beside speed
  (S10.1); the separate file exists so a consumer that strips the truth column still
  has the labels S8.5's metrics need.
* **A `(personas, records)` pair determines the corpus.** Records are dealt to
  personas as evenly as the pair allows -- `records // personas` each, with the
  remainder going to the lowest-numbered personas -- and each persona's records are
  dealt round-robin across the three sources from an offset that is the persona's own
  index. Every persona therefore holds at least one record (so the distinct
  `persona_id` count IS `--personas`) and no source is systematically favoured.
* **Nothing is drawn from a stream shared with another row.** Each row's two RNGs
  come from :func:`~generator.corruptions.record_rng` over
  `(seed, source, persona index, ordinal)`, and the layout draws (`updated_at`) use
  a *different* stream name from the corruption draws. Adding a corruption axis
  therefore cannot move a timestamp, and writing a `--batch` delivery does not
  require replaying the base corpus to reach the right point in a stream.

The module writes only under the caller's ``out_dir``. `fixtures/static/` is
hand-authored (S8.2) and is a `protected_paths` entry of this ticket.
"""

from __future__ import annotations

import csv
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import date, datetime, timedelta
from pathlib import Path
from types import MappingProxyType
from typing import Final

from er.config.loader import load_config
from er.config.schema import Config, SourceSpec

from .corruptions import CorruptionProfile, corrupt_record, load_profiles, record_rng
from .personas import Persona

__all__ = [
    "BATCH_DIRNAME",
    "CANONICAL_FIELDS",
    "DEFAULT_CONFIG_PATH",
    "DEFAULT_HOUSEHOLD_RATE",
    "SOURCE_HEADERS",
    "SOURCE_ORDER",
    "SOURCE_RECORD_ID_PREFIXES",
    "TRUTH_FILENAME",
    "TRUTH_HEADER",
    "CorpusSpec",
    "emit_corpus",
    "source_header",
]

_REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: S6 calls `configs/test.yaml` "the file the fixtures and CI use verbatim", so it is
#: the default document the corpus shape is read from. A benchmark passes its own.
DEFAULT_CONFIG_PATH: Final = _REPO_ROOT / "configs" / "test.yaml"

#: The three sources of S6, in `priority_rank` order. The round-robin deal walks this
#: tuple, so its ORDER is part of the corpus and not merely a listing.
SOURCE_ORDER: Final = ("crm", "billing", "webforms")

#: The nine canonical attributes of V11, in the column order `base_10` carries them.
#: `source_header` interleaves this with the source's own id and timestamp columns.
CANONICAL_FIELDS: Final = (
    "given_name",
    "family_name",
    "email",
    "phone",
    "address_line",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
)

#: The truth label S8.2 makes the last field of every input row.
TRUTH_COLUMN: Final = "persona_id"

#: The three header tuples, literally as S8.2 states them. This is the SINGLE
#: definition in the repo: `tests/unit/generator/test_emit.py` reads the committed
#: `base_10` CSVs and asserts each first line equals its entry here, and
#: `emit_corpus` asserts each equals what `source_header` derives from S6 -- so the
#: fixture, the spec's column mapping and the generator are pinned to one another
#: and a change to any of the three fails rather than diverging.
SOURCE_HEADERS: Final[Mapping[str, tuple[str, ...]]] = MappingProxyType(
    {
        "crm": (
            "crm_id",
            "first_name",
            "last_name",
            "email_address",
            "phone",
            "street_address",
            "city",
            "state",
            "zip",
            "dob",
            "last_modified",
            "persona_id",
        ),
        "billing": (
            "account_no",
            "fname",
            "lname",
            "billing_email",
            "contact_phone",
            "addr1",
            "addr_city",
            "addr_state",
            "postal_code",
            "birth_date",
            "updated_ts",
            "persona_id",
        ),
        "webforms": (
            "submission_id",
            "name_first",
            "name_last",
            "email",
            "phone_number",
            "address",
            "city",
            "region",
            "postcode",
            "date_of_birth",
            "submitted_at",
            "persona_id",
        ),
    }
)

#: `truth.csv`, in the column order `fixtures/static/base_10/truth.csv` uses, so one
#: reader serves the hand-authored scenarios and the generated corpora both.
TRUTH_HEADER: Final = ("persona_id", "source_system", "source_record_id")
TRUTH_FILENAME: Final = "truth.csv"

#: An incremental delivery goes in this subdirectory of `out_dir`, which is the
#: `batch` of S8.2.1's phase vocabulary. The base delivery is written at the root of
#: `out_dir` rather than in a `base/` subdirectory because `storage.drop_dir` (S6) is
#: where `er ingest` looks, and a benchmark drops the base corpus straight into it.
BATCH_DIRNAME: Final = "batch"

#: `<prefix><8 digits>`, per source. Fixed width for `personas.py`'s reason: the id
#: of the seventh `crm` record is `C00000007` at every scale, so a `truth.csv` from
#: one scale cannot silently be read against another.
SOURCE_RECORD_ID_PREFIXES: Final[Mapping[str, str]] = MappingProxyType(
    {"crm": "C", "billing": "B", "webforms": "W"}
)
RECORD_ID_WIDTH: Final = 8

#: The share of personas sharing an address (`personas.py`). Not a CLI argument: it
#: is a property of the corpus SHAPE rather than of a scale, and S10.2's table has no
#: column for it, so pinning it here keeps two scales comparable on it.
DEFAULT_HOUSEHOLD_RATE: Final = 0.1

#: `updated_at` is drawn uniformly from a fixed window starting here. A literal
#: rather than a clock: S10.1 forbids reading one, and a corpus whose timestamps
#: moved with the wall clock would make `COALESCE(updated_at_source, ingested_at)` --
#: the recency rule of S6 survivorship -- decide differently on two runs of the same
#: seed. The window is wide enough that two records of one persona rarely tie, which
#: is what keeps the tie-break path (S4.6) an exception rather than the common case.
UPDATED_AT_EPOCH: Final = datetime(2024, 1, 1, 0, 0, 0)
UPDATED_AT_SPAN_SECONDS: Final = 730 * 24 * 60 * 60
#: What `base_10` writes in `last_modified` / `updated_ts` / `submitted_at`. Distinct
#: from `sources.<name>.date_format`, which S6 applies to the `birth_date` column
#: alone -- the two are different columns and a shared format would be a coincidence.
UPDATED_AT_FORMAT: Final = "%Y-%m-%d %H:%M:%S"

#: `csv.writer` defaults to CRLF. The corpus is compared byte-for-byte across
#: processes and platforms (S10.1), so the terminator is stated rather than inherited.
_LINE_TERMINATOR: Final = "\n"


@dataclass(frozen=True, slots=True)
class CorpusSpec:
    """The shape of one generated corpus: an S10.2 scale row, as arguments.

    `benchmarks/scales.yaml` does not exist until M5, so a scale reaches the
    generator as these fields and never as a file read.
    """

    seed: int
    personas: int
    records: int
    #: Rows in the incremental `batch/` delivery; ``0`` emits none.
    batch: int = 0
    household_rate: float = DEFAULT_HOUSEHOLD_RATE

    def __post_init__(self) -> None:
        if self.personas < 1:
            raise ValueError(f"personas must be >= 1, got {self.personas}")
        if self.records < self.personas:
            # Every persona MUST hold at least one record: the distinct `persona_id`
            # count is the corpus's entity count, and a persona with no record is a
            # ground-truth entity the pipeline is graded on and never shown.
            raise ValueError(
                f"records must be >= personas so every persona is represented, "
                f"got records={self.records} personas={self.personas}"
            )
        if self.batch < 0:
            raise ValueError(f"batch must be >= 0, got {self.batch}")
        if not 0.0 <= self.household_rate <= 1.0:
            raise ValueError(f"household_rate must be within [0, 1], got {self.household_rate}")


def source_header(spec: SourceSpec) -> tuple[str, ...]:
    """The header S6's column mapping implies for one source (S8.2, M14).

    `record_id_column`, then the nine canonical attributes in :data:`CANONICAL_FIELDS`
    order, then `updated_at_column`, then the truth column. S8.2 states the same
    shape as "the first column is the source's `record_id_column`; the second-to-last
    is its `updated_at_column`; `persona_id` ... is the last field of every row".
    """
    return (
        spec.record_id_column,
        *(spec.columns[field] for field in CANONICAL_FIELDS),
        spec.updated_at_column,
        TRUTH_COLUMN,
    )


def _check_headers(config: Config) -> None:
    """Refuse to emit under a config whose columns disagree with :data:`SOURCE_HEADERS`."""
    missing = sorted(set(SOURCE_ORDER) - set(config.sources))
    if missing:
        raise ValueError(f"the config does not define source(s) {missing}; expected {SOURCE_ORDER}")
    for source in SOURCE_ORDER:
        derived = source_header(config.sources[source])
        if derived != SOURCE_HEADERS[source]:
            raise ValueError(
                f"sources.{source}.columns implies header {derived}, but S8.2's committed "
                f"header is {SOURCE_HEADERS[source]}; S6 owns the mapping, so fix the config "
                f"or the fixture together"
            )


def _counts_per_persona(persona_count: int, records: int) -> list[int]:
    """Deal ``records`` over ``persona_count`` personas as evenly as the pair allows.

    The remainder goes to the lowest-numbered personas rather than to a random
    subset: which personas are duplicated more is then a function of the
    `(personas, records)` pair alone, so two scales with the same ratio have the same
    duplication shape and S10.4 can compare them.
    """
    base, remainder = divmod(records, persona_count)
    return [base + (1 if index < remainder else 0) for index in range(persona_count)]


def _deal(counts: Sequence[int], offsets: Sequence[int]) -> list[tuple[int, int, str]]:
    """`(persona index, ordinal, source)` for every row of one delivery.

    ``offsets[i]`` is the number of records persona ``i`` already holds, so a `batch/`
    delivery continues each persona's ordinals instead of re-issuing them -- two rows
    of one persona with the same ordinal would draw the same corruptions from the
    same stream and be byte-identical apart from their record id.
    """
    rows: list[tuple[int, int, str]] = []
    for index, count in enumerate(counts):
        for step in range(count):
            ordinal = offsets[index] + step
            rows.append((index, ordinal, SOURCE_ORDER[(index + ordinal) % len(SOURCE_ORDER)]))
    return rows


def _updated_at(seed: int, source: str, persona_index: int, ordinal: int) -> str:
    """This row's `updated_at`, from a stream disjoint from the corruption draws."""
    rng = record_rng(seed, f"{source}:updated_at", persona_index, ordinal)
    moment = UPDATED_AT_EPOCH + timedelta(seconds=rng.randrange(UPDATED_AT_SPAN_SECONDS))
    return moment.strftime(UPDATED_AT_FORMAT)


def _render_birth_date(birth_date: date | None, date_format: str) -> str:
    """The `birth_date` cell: the source's `date_format`, or an empty field.

    Never the literal `None`: S8.2's missing-value convention for an INPUT row is an
    empty field, and the string `None` would reach `stg_*` as a value that parses to
    NULL only by accident of the format failing.
    """
    return "" if birth_date is None else birth_date.strftime(date_format)


def _row(
    record_id: str,
    persona: Persona,
    profile: CorruptionProfile,
    seed: int,
    source: str,
    persona_index: int,
    ordinal: int,
    date_format: str,
) -> tuple[str, ...]:
    """One CSV row, in :func:`source_header` order."""
    record = corrupt_record(persona, profile, record_rng(seed, source, persona_index, ordinal))
    return (
        record_id,
        record.given_name,
        record.family_name,
        record.email,
        record.phone,
        record.address_line,
        record.addr_city,
        record.addr_region,
        record.addr_postal,
        _render_birth_date(record.birth_date, date_format),
        _updated_at(seed, source, persona_index, ordinal),
        record.persona_id,
    )


def _write_csv(path: Path, header: Sequence[str], rows: Sequence[Sequence[str]]) -> None:
    """Write one CSV with the pinned terminator and no trailing state."""
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator=_LINE_TERMINATOR)
        writer.writerow(header)
        writer.writerows(rows)


def _write_delivery(
    out_dir: Path,
    deal: Sequence[tuple[int, int, str]],
    personas: Sequence[Persona],
    profiles: Mapping[str, CorruptionProfile],
    config: Config,
    seed: int,
    next_id: dict[str, int],
) -> list[Path]:
    """Emit one delivery's three source CSVs plus its `truth.csv`.

    ``next_id`` is carried across deliveries so a `batch/` record id continues the
    base corpus's sequence; a restarted sequence would give two different records the
    same `(source_system, source_record_id)`, which S5.0 makes one record's key.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    rows: dict[str, list[tuple[str, ...]]] = {source: [] for source in SOURCE_ORDER}
    truth: list[tuple[str, str, str]] = []

    for persona_index, ordinal, source in deal:
        record_id = f"{SOURCE_RECORD_ID_PREFIXES[source]}{next_id[source]:0{RECORD_ID_WIDTH}d}"
        next_id[source] += 1
        persona = personas[persona_index]
        rows[source].append(
            _row(
                record_id,
                persona,
                profiles[source],
                seed,
                source,
                persona_index,
                ordinal,
                config.sources[source].date_format,
            )
        )
        truth.append((persona.persona_id, source, record_id))

    written: list[Path] = []
    for source in SOURCE_ORDER:
        path = out_dir / f"{source}.csv"
        _write_csv(path, SOURCE_HEADERS[source], rows[source])
        written.append(path)

    truth_path = out_dir / TRUTH_FILENAME
    # Sorted on the full column tuple, which is S8.2.1's rule for every committed
    # truth file: a corpus regenerated with a reordered deal then produces a
    # byte-identical truth set, so a diff here means the LABELS changed.
    _write_csv(truth_path, TRUTH_HEADER, sorted(truth))
    written.append(truth_path)
    return written


def emit_corpus(
    spec: CorpusSpec,
    personas: Sequence[Persona],
    out_dir: Path | str,
    *,
    config: Config | None = None,
    profiles: Mapping[str, CorruptionProfile] | None = None,
) -> list[Path]:
    """Write the corpus ``spec`` describes under ``out_dir``.

    Args:
        spec: the scale shape; `seed` is `generator.seed` (S6) unless the caller
            overrode it.
        personas: the ground truth, from `generate_personas(spec.seed,
            spec.personas, spec.household_rate)`. Passed in rather than drawn here so
            a caller can emit two deliveries against one truth set.
        out_dir: the directory to write into; created if absent. Only this directory
            and its `batch/` child are written.
        config: the validated S6 document; `configs/test.yaml` when omitted.
        profiles: the corruption profiles; `profiles.yaml` when omitted.

    Returns:
        Every file written, base delivery first, in the order it was written.

    Raises:
        ValueError: ``personas`` does not hold `spec.personas` entries, or the
            config's column mapping disagrees with :data:`SOURCE_HEADERS`.
    """
    if len(personas) != spec.personas:
        raise ValueError(
            f"spec.personas is {spec.personas} but {len(personas)} personas were supplied"
        )
    resolved_config = config if config is not None else load_config(DEFAULT_CONFIG_PATH)
    _check_headers(resolved_config)
    resolved_profiles = profiles if profiles is not None else load_profiles(resolved_config)

    out_path = Path(out_dir)
    next_id = dict.fromkeys(SOURCE_ORDER, 1)

    base_counts = _counts_per_persona(spec.personas, spec.records)
    written = _write_delivery(
        out_path,
        _deal(base_counts, [0] * spec.personas),
        personas,
        resolved_profiles,
        resolved_config,
        spec.seed,
        next_id,
    )

    if spec.batch:
        batch_counts = _counts_per_persona(spec.personas, spec.batch)
        written.extend(
            _write_delivery(
                out_path / BATCH_DIRNAME,
                _deal(batch_counts, base_counts),
                personas,
                resolved_profiles,
                resolved_config,
                spec.seed,
                next_id,
            )
        )
    return written
