"""Pydantic models for the S6 configuration document (DesignDoc.md S6, S6.1).

The document is the single source of truth for every tunable the pipeline reads,
so three things are normative here and are stated nowhere else in Python:

* Every model is ``extra='forbid'``. A key the spec does not name is a typo, and a
  typo that validates is a tunable silently taking its default.
* Each validator below raises a ``ValueError`` whose message *begins with the S6.1
  failure message key*, so the loader can surface the key without a table mapping
  error types to keys. Only the single-block validators V9-V16 live here; the
  cross-field and referential validators V1-V8 belong to the ticket that owns
  ``columns.unknown`` and the survivorship keyset.
* :func:`normalize` is an explicit step the loader invokes AFTER validation, never
  a field validator: V6 and V8 inspect the pre-normalized ``levels`` lists, and
  ``config_hash`` is computed over the post-normalized document (S6.1, S5.2).
"""

from __future__ import annotations

import re
from typing import Self

from pydantic import BaseModel, ConfigDict, model_validator

__all__ = [
    "CANONICAL_ATTRIBUTES",
    "CONFIG_BLOCKS",
    "TERMINAL_SURVIVORSHIP_RULE",
    "BlockingRule",
    "Clustering",
    "Coherence",
    "ComparisonSpec",
    "Config",
    "CorrectionPass",
    "Generator",
    "SourceSpec",
    "Standardization",
    "Storage",
    "Thresholds",
    "Training",
    "TrainingEm",
    "Versions",
    "normalize",
]

#: The fourteen top-level blocks of S6, in document order. ``Config.model_fields``
#: MUST equal this set; the test that asserts it is what keeps a fifteenth block
#: from arriving without a spec row.
CONFIG_BLOCKS: tuple[str, ...] = (
    "tenant",
    "thresholds",
    "standardization",
    "sources",
    "blocking",
    "comparisons",
    "survivorship",
    "training",
    "storage",
    "versions",
    "generator",
    "clustering",
    "coherence",
    "correction_pass",
)

#: The canonical attributes the S4.2 standardization macros consume; every
#: ``sources.<name>.columns`` mapping MUST cover all of them (V11).
CANONICAL_ATTRIBUTES: tuple[str, ...] = (
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

#: S6.1 appends this as the terminal element of every survivorship chain, which is
#: what makes each chain a total order over the records of an entity.
TERMINAL_SURVIVORSHIP_RULE = "record_key ASC"

# Syntactic only, deliberately: V16 asks whether a placeholder could be an address
# `email_norm` will see, not whether it is deliverable.
_EMAIL_RE = re.compile(r"[^@\s]+@[^@\s]+\.[^@\s]+")

#: ISO 3166-1 alpha-2, the form `phone_e164` passes to the phone parser (S4.2).
_REGION_RE = re.compile(r"[A-Z]{2}")

# minute, hour, day-of-month, month, day-of-week. 7 is Sunday's second spelling,
# which the standard 5-field cron accepts alongside 0.
_CRON_FIELD_BOUNDS: tuple[tuple[int, int], ...] = ((0, 59), (0, 23), (1, 31), (1, 12), (0, 7))


def _is_cron_field(field: str, low: int, high: int) -> bool:
    """Whether one 5-field-cron field is well formed within ``[low, high]``.

    Hand-written because S2.1 pins no cron library and adding one to parse a single
    config string would be a dependency the pin table has to carry forever. The
    grammar accepted is the portable one: comma-separated items, each ``*``, a
    number, or a ``a-b`` range, any of them optionally followed by ``/step``.
    """
    for item in field.split(","):
        expression, _, step = item.partition("/")
        if step and (not step.isdigit() or int(step) < 1):
            return False
        if expression == "*":
            continue
        start, dash, end = expression.partition("-")
        if dash and not end:
            return False
        for value in (start, end) if dash else (start,):
            if not value.isdigit() or not low <= int(value) <= high:
                return False
        if dash and int(start) > int(end):
            return False
    return True


class _Block(BaseModel):
    """Common configuration for every model in the document."""

    model_config = ConfigDict(extra="forbid")


class Thresholds(_Block):
    """S6 `thresholds`. The units and the gray band are normative in S4.3."""

    auto_merge: float
    review_low: float


class Standardization(_Block):
    """S6 `standardization`. Consumed by `email_norm` / `phone_e164` and nothing else."""

    email_strip_plus_addressing: bool
    email_placeholders: list[str]
    phone_default_region: str

    @model_validator(mode="after")
    def _v16_placeholders_and_region(self) -> Self:
        for placeholder in self.email_placeholders:
            if placeholder != placeholder.lower():
                raise ValueError(
                    f"standardization.invalid: email_placeholders must be lowercase, "
                    f"got {placeholder!r}"
                )
            if not _EMAIL_RE.fullmatch(placeholder):
                raise ValueError(
                    f"standardization.invalid: {placeholder!r} is not a syntactically "
                    f"valid email address"
                )
        duplicates = sorted(
            {p for p in self.email_placeholders if self.email_placeholders.count(p) > 1}
        )
        if duplicates:
            raise ValueError(
                f"standardization.invalid: duplicate email_placeholders entries {duplicates}"
            )
        if not _REGION_RE.fullmatch(self.phone_default_region):
            raise ValueError(
                f"standardization.invalid: phone_default_region must be a 2-letter "
                f"ISO 3166-1 alpha-2 code, got {self.phone_default_region!r}"
            )
        return self


class SourceSpec(_Block):
    """S6 `sources.<name>`: one delivering system and its column mapping."""

    adapter: str
    priority_rank: int
    record_id_column: str
    updated_at_column: str
    date_format: str
    #: canonical attribute -> the column that carries it in this source. Extra
    #: entries are allowed; the nine canonical ones are mandatory (V11).
    columns: dict[str, str]

    @model_validator(mode="after")
    def _v11_columns_complete(self) -> Self:
        missing = [attribute for attribute in CANONICAL_ATTRIBUTES if attribute not in self.columns]
        if missing:
            raise ValueError(f"sources.columns.incomplete: columns does not map {missing}")
        if self.priority_rank < 1:
            raise ValueError(
                f"sources.columns.incomplete: priority_rank must be a positive integer, "
                f"got {self.priority_rank}"
            )
        return self


class BlockingRule(_Block):
    """S6 `blocking[]`: one candidate-generation key and the expression that builds it."""

    key_type: str
    expr: str


class ComparisonSpec(_Block):
    """S6 `comparisons.<column>`: the comparison levels and whether TF adjustment applies."""

    #: ``None`` is the YAML `null` token, which S6.1 normalization removes; the
    #: comparison builder always emits `NullLevel` first regardless.
    levels: list[str | None]
    tf: bool = False


class TrainingEm(_Block):
    """S6 `training.em`: the arguments of every EM session (S4.3.2)."""

    fix_u_probabilities: bool


class Training(_Block):
    """S6 `training`. Every argument of the S4.3.2 call sequence comes from here."""

    deterministic_rules: list[str]
    recall: float
    u_max_pairs: int
    #: REQUIRED with no default (V10): a defaulted seed makes the byte-equality
    #: claim of the training determinism test unreproducible.
    u_seed: int
    em_blocking_rules: list[str]
    em: TrainingEm

    @model_validator(mode="after")
    def _v9_rule_counts(self) -> Self:
        # m is not estimated for a blocked column, so one session leaves the
        # blocked column's m at its prior (S4.3.2).
        if len(self.em_blocking_rules) < 2:
            raise ValueError(
                f"training.em_blocking_rules.min_items: at least 2 EM sessions are "
                f"required, got {len(self.em_blocking_rules)}"
            )
        if not self.deterministic_rules:
            raise ValueError(
                "training.em_blocking_rules.min_items: at least 1 deterministic rule is required"
            )
        return self

    @model_validator(mode="after")
    def _v10_sampling_bounds(self) -> Self:
        # V10's row covers three assertions under one message key. A missing
        # `u_seed` produces that key from Pydantic's own `missing` error (the
        # loader derives it from the error path), and these two bounds carry the
        # same row key so a reader can find the rule from any of its failures.
        if self.u_max_pairs < 1:
            raise ValueError(
                f"training.u_seed.required: u_max_pairs must be >= 1, got {self.u_max_pairs}"
            )
        if not 0 < self.recall <= 1:
            raise ValueError(
                f"training.u_seed.required: recall must satisfy 0 < recall <= 1, got {self.recall}"
            )
        return self


class Storage(_Block):
    """S6 `storage`: where the lake, the drop folder and the model artifacts live."""

    data_path: str
    drop_dir: str
    #: Already ends in `/` and already names the tenant (V14); S4.3.2 forbids
    #: interpolating the tenant a second time when building a model URI.
    model_uri_prefix: str

    @model_validator(mode="after")
    def _v14_uris(self) -> Self:
        for name in ("data_path", "model_uri_prefix"):
            value: str = getattr(self, name)
            if not value.startswith("s3://") or not value.endswith("/"):
                raise ValueError(
                    f"storage.uri: {name} must be an s3:// URI ending in '/', got {value!r}"
                )
        if not self.drop_dir.startswith("/"):
            raise ValueError(
                f"storage.uri: drop_dir must be an absolute path, got {self.drop_dir!r}"
            )
        return self


class Versions(_Block):
    """S6 `versions`: the version strings the CLI passes to dbt as `--vars` overrides."""

    std_version: str
    survivorship_version: str
    address_parser_version: str

    @model_validator(mode="after")
    def _v13_non_empty(self) -> Self:
        for name in ("std_version", "survivorship_version", "address_parser_version"):
            value: str = getattr(self, name)
            if not value.strip():
                raise ValueError(f"versions.required: {name} must be a non-empty string")
        return self


class Generator(_Block):
    """S6 `generator`: the seed of the synthetic-data generator (S8, S10)."""

    seed: int


class Clustering(_Block):
    """S6 `clustering`: the label-propagation bounds (S4.5)."""

    max_iterations: int
    cut_protect_probability: float

    @model_validator(mode="after")
    def _v12_bounds(self) -> Self:
        if self.max_iterations < 1:
            raise ValueError(
                f"clustering.bounds: max_iterations must be >= 1, got {self.max_iterations}"
            )
        if not 0 < self.cut_protect_probability <= 1:
            raise ValueError(
                f"clustering.bounds: cut_protect_probability must satisfy 0 < p <= 1, "
                f"got {self.cut_protect_probability}"
            )
        return self


class Coherence(_Block):
    """S6 `coherence`: the phase-2 embedding scorer; `noop` in v1 (S11)."""

    scorer: str


class CorrectionPass(_Block):
    """S6 `correction_pass`: when the periodic full-corpus correction runs (S4.7)."""

    cadence: str

    @model_validator(mode="after")
    def _v15_cadence(self) -> Self:
        fields = self.cadence.split()
        if len(fields) != len(_CRON_FIELD_BOUNDS) or not all(
            _is_cron_field(field, low, high)
            for field, (low, high) in zip(fields, _CRON_FIELD_BOUNDS, strict=True)
        ):
            raise ValueError(
                f"correction_pass.cadence: cadence must be a 5-field cron expression, "
                f"got {self.cadence!r}"
            )
        return self


class Config(_Block):
    """The whole S6 document, validated. `config_hash` is NOT a field of it (S6)."""

    tenant: str
    thresholds: Thresholds
    standardization: Standardization
    sources: dict[str, SourceSpec]
    blocking: list[BlockingRule]
    comparisons: dict[str, ComparisonSpec]
    survivorship: dict[str, list[str]]
    training: Training
    storage: Storage
    versions: Versions
    generator: Generator
    clustering: Clustering
    coherence: Coherence
    correction_pass: CorrectionPass

    @model_validator(mode="after")
    def _v11_priority_ranks_unique(self) -> Self:
        # Cross-source, so it cannot live on SourceSpec: `priority_rank` is the
        # source_priority survivorship rule's total order, and a tie there would
        # push the decision to the terminal `record_key ASC` at random.
        ranks = [source.priority_rank for source in self.sources.values()]
        duplicated = sorted({rank for rank in ranks if ranks.count(rank) > 1})
        if duplicated:
            raise ValueError(
                f"sources.columns.incomplete: priority_rank values must be unique across "
                f"sources, {duplicated} repeat"
            )
        return self


def normalize(config: Config) -> Config:
    """Apply the S6.1 post-validation normalization and return a new document.

    Two rewrites, both part of the canonicalised document `config_hash` covers, so
    two configs differing only in a redundant `null` token hash identically:

    * the `null` token is dropped from every `comparisons[*].levels` list, and
    * `record_key ASC` becomes the terminal element of every survivorship chain.

    The loader calls this after validation. Calling it earlier would hide the
    pre-normalized lists from the validators that read them (V6, V8).
    """
    normalized = config.model_copy(deep=True)
    for comparison in normalized.comparisons.values():
        comparison.levels = [level for level in comparison.levels if level is not None]
    for attribute, chain in normalized.survivorship.items():
        if not chain or chain[-1] != TERMINAL_SURVIVORSHIP_RULE:
            normalized.survivorship[attribute] = [*chain, TERMINAL_SURVIVORSHIP_RULE]
    return normalized
