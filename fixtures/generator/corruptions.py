"""Per-source corruption profiles for the seeded synthetic corpus (DesignDoc.md S10.1).

`personas.py` produces the truth; this module produces what a source system would
actually be holding about it. S10.1 names five axes -- "typo rate, nickname
substitution, per-field missingness, format drift, stale addresses" -- and each one
is a rate on :class:`CorruptionProfile`, read from `profiles.yaml` rather than
written into the code, so the shape of the corpus is data a benchmark can restate.

Five decisions here are load-bearing, and each one exists because the alternative
would quietly destroy something a test downstream is graded on:

* **Every draw is `randrange` over integers.** S10.1 forbids a clock and an unseeded
  RNG, and `personas.py` states the stronger reason: determinism has to hold ACROSS
  PROCESSES. A rate is a float in the YAML because that is how a rate reads, but it
  is converted once to an integer numerator over :data:`RATE_SCALE` and every trial
  is `rng.randrange(RATE_SCALE) < numerator`, so no float comparison and no float
  summation ever decides a corpus value.
* **The nickname vocabulary is the dbt seed and nothing else.** S4.2's
  `variant_match` compares through `name_variants`, which is backed by
  `dbt/seeds/nickname_variants.csv` (ER-039). A diminutive invented here would be a
  true pair the pipeline is *unable* to find, so it would read as a recall
  regression in the S10.5 quality numbers with nothing in the pipeline to fix.
* **`given_name` carries no typo, and its two substitution axes never compose.** A
  typo on top of a substitution leaves a value that is neither the persona's name
  nor a justifiable alternate, and "did the generator emit a name it could not
  justify?" stops being answerable from the corpus. Typos therefore land on
  `family_name`, which is also where `base_10`'s typo trap lives (S8.2) and where
  the `jaro_winkler:0.90` level of S6 is the intended catcher. The nickname axis
  substitutes a seed variant; the *alternate-name* axis (`alt_given_rate`) draws a
  different legal name from the committed weighted list -- the middle-name /
  goes-by shape real sources hold -- and it fires only when the nickname axis did
  not, so every emitted given name is the persona's, a seed variant of it, or a
  member of the committed vocabulary.
* **Only `email`, `phone` and `birth_date` may go missing.** S8.2's missing-email
  trap is exactly this axis. `address_line`, the six `addr_*` values and the two
  name columns are excluded by :data:`MISSABLE_FIELDS` and rejected by
  :func:`load_profiles`, because `name_postal` (S6 blocking) is built from
  `family_name` and `addr_postal`: nulling either does not corrupt a record, it
  removes it from candidate generation altogether, and a record no rule pairs is a
  missing edge no threshold change can recover.
* **A stale address stays inside its postal area; a move leaves it.** S10.1's
  stale-address axis is drift within a life: a different house number and street
  with the same city, region and postal code, so `name_postal` still pairs the
  record. The *moved* axis (`moved_postal_rate`) is a person who left the area --
  a full new address with a different postal code -- and it exists because the
  `addr_postal` comparison of S6 has a disagreement level that EM can only fit if
  two records of one persona ever disagree on postal; without it that level has no
  support, the fitted m is undefined, and Splink drops the whole comparison at
  predict time. A moved record still pairs through `email_exact`, `phone_exact`
  or `dob_name`, so the corpus loses no persona from candidate generation.
* **Within-persona field drift is a real shape, and every S6 comparison level must
  have support.** Sources genuinely disagree about a person: a billing address on
  an old email, a webform typed with a new phone number, a birthday keyed a day
  off. The `alt_email_rate`, `email_domain_swap_rate` (same username, different
  domain -- the `username_exact` level's shape), `alt_phone_rate`,
  `dob_day_typo_rate` (same year and month -- `dob_same_year_month`'s shape) and
  `dob_wrong_rate` axes emit exactly those disagreements. They are what gives EM
  support for every disagreement level of S6's comparisons; a corpus in which
  matches never disagree on a field trains that field's disagreement m to nothing,
  and the committed fixture model then scores every real-world disagreement as
  impossible.

The module reads two committed files -- the nickname seed and `profiles.yaml` -- and
performs no other I/O.
"""

from __future__ import annotations

import csv
import hashlib
import random
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date, timedelta
from functools import cache
from pathlib import Path
from types import MappingProxyType
from typing import Any, Final

import yaml

from er.config.schema import Config

from .personas import (
    AREA_CODES,
    CITIES,
    EARLIEST_BIRTH_DATE,
    EMAIL_DOMAINS,
    FICTIONAL_EXCHANGE,
    LATEST_BIRTH_DATE,
    MAX_HOUSE_NUMBER,
    POSTAL_SPREAD,
    STREET_NAMES,
    STREET_TYPES,
    Persona,
    given_names,
)

__all__ = [
    "MISSABLE_FIELDS",
    "NICKNAME_SEED_PATH",
    "PHONE_FORMS",
    "PROFILES_PATH",
    "RATE_SCALE",
    "CorruptedRecord",
    "CorruptionProfile",
    "corrupt_record",
    "load_profiles",
    "nickname_pairs",
    "record_rng",
    "render_phone",
]

_REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: ER-039's seed, and the single vocabulary `variant_match` (S4.2) can compare
#: through. Read, never written: it is a `protected_paths` entry of this ticket.
NICKNAME_SEED_PATH: Final = _REPO_ROOT / "dbt" / "seeds" / "nickname_variants.csv"

#: The committed profile document, beside the code that validates it.
PROFILES_PATH: Final = Path(__file__).resolve().parent / "profiles.yaml"

#: A rate is applied as `randrange(RATE_SCALE) < round(rate * RATE_SCALE)`. Four
#: decimal places is finer than any rate a corpus shape needs and coarse enough that
#: the numerator is exact in `int`, which is what keeps a trial off floating point.
RATE_SCALE: Final = 10_000

#: The three phone surfaces of S8.2's drifted-phone trap, by name. `e164` is the
#: persona's own spelling; the other two are what a source system holding a US
#: number typically stores.
PHONE_FORMS: Final = ("e164", "dashed", "parens")

#: The only fields a profile may null. See the module docstring: the excluded ones
#: are blocking-key inputs, and nulling those removes a record from candidate
#: generation rather than corrupting it.
MISSABLE_FIELDS: Final = ("email", "phone", "birth_date")

#: Every rate name on a profile, so an unknown key in the YAML is a load error and
#: not a silently ignored line.
_RATE_FIELDS: Final = (
    "typo_rate",
    "nickname_rate",
    "alt_given_rate",
    "stale_address_rate",
    "moved_postal_rate",
    "alt_email_rate",
    "email_domain_swap_rate",
    "alt_phone_rate",
    "dob_day_typo_rate",
    "dob_wrong_rate",
)

#: The keyboard-adjacency substitutions a typo may make, plus the two structural
#: typos (transposition and deletion) `_apply_typo` chooses between. Adjacency
#: rather than a uniform letter draw: `jaro_winkler` scores a near-miss on a long
#: surname well above a random letter swap, and S6 sets that level at 0.90, so a
#: uniform draw would push most typo pairs below the level meant to catch them.
_ADJACENT: Final[Mapping[str, str]] = MappingProxyType(
    {
        "a": "sq",
        "b": "vn",
        "c": "xv",
        "d": "sf",
        "e": "wr",
        "f": "dg",
        "g": "fh",
        "h": "gj",
        "i": "uo",
        "j": "hk",
        "k": "jl",
        "l": "k",
        "m": "n",
        "n": "bm",
        "o": "ip",
        "p": "o",
        "q": "wa",
        "r": "et",
        "s": "ad",
        "t": "ry",
        "u": "yi",
        "v": "cb",
        "w": "qe",
        "x": "zc",
        "y": "tu",
        "z": "x",
    }
)

#: A name shorter than this is left alone: deleting or transposing a character of a
#: three-letter surname produces a value `jaro_winkler:0.90` cannot recover, which
#: is a missed edge rather than the fuzzy-match exercise the axis is for.
MIN_TYPO_LENGTH: Final = 5


@dataclass(frozen=True, slots=True)
class CorruptionProfile:
    """One source system's drift, as `profiles.yaml` states it.

    Rates are the fraction of records the axis fires on. They are floats because a
    rate reads as one; :attr:`_numerator` converts each to the integer form every
    trial actually uses.
    """

    source: str
    typo_rate: float
    nickname_rate: float
    #: A different legal given name from the committed weighted list -- the
    #: middle-name / goes-by shape. Fires only when the nickname axis did not.
    alt_given_rate: float
    stale_address_rate: float
    #: A full new address with a different postal code -- the person left the
    #: area. Takes precedence over the stale axis when both fire.
    moved_postal_rate: float
    #: A different email entirely -- a second mailbox the source holds.
    alt_email_rate: float
    #: The persona's username on a different domain -- `username_exact`'s shape.
    #: Fires only when the alternate-email axis did not.
    email_domain_swap_rate: float
    #: A different phone number entirely -- an old or secondary line.
    alt_phone_rate: float
    #: The persona's birthday keyed a day off, inside the same year and month.
    dob_day_typo_rate: float
    #: A wholly wrong birth date. Fires only when the day-typo axis did not.
    dob_wrong_rate: float
    #: field -> the fraction of records emitting it empty. Keys are a subset of
    #: :data:`MISSABLE_FIELDS`; an absent key means the field never goes missing.
    missing_rates: Mapping[str, float]
    #: phone form -> integer weight. Integer, for the reason `personas.py` gives
    #: about weighted draws: a float cumulative weight makes the draw depend on
    #: summation order.
    phone_form_weights: Mapping[str, int]

    def fires(self, rate: float, rng: random.Random) -> bool:
        """One Bernoulli trial at ``rate``, decided in integer arithmetic."""
        return rng.randrange(RATE_SCALE) < round(rate * RATE_SCALE)

    def missing_rate(self, field: str) -> float:
        """The configured missingness of ``field``; ``0.0`` when unstated."""
        return self.missing_rates.get(field, 0.0)

    def pick_phone_form(self, rng: random.Random) -> str:
        """One weighted draw over the configured phone surfaces."""
        total = sum(self.phone_form_weights.values())
        cut = rng.randrange(total)
        for form in PHONE_FORMS:
            weight = self.phone_form_weights.get(form, 0)
            if cut < weight:
                return form
            cut -= weight
        # Unreachable: `cut < total` and the weights sum to `total`. Raising rather
        # than falling through to a default keeps a future weight-map edit from
        # silently making one form the answer for every draw.
        raise RuntimeError(f"{self.source}: phone form weights do not cover the draw")


@dataclass(frozen=True, slots=True)
class CorruptedRecord:
    """One source system's view of one persona, ready for `emit.py` to lay out.

    Every field is the surface string the CSV carries, with two exceptions that are
    deliberate: an empty string is the missing value (S8.2's missing-email trap is
    an empty field, not a sentinel), and `birth_date` stays a :class:`date` because
    only `emit.py` knows the source's `date_format` (S6).
    """

    persona_id: str
    given_name: str
    family_name: str
    email: str
    phone: str
    address_line: str
    addr_city: str
    addr_region: str
    addr_postal: str
    birth_date: date | None


@cache
def nickname_pairs() -> frozenset[frozenset[str]]:
    """Every `{a, b}` pair of the ER-039 seed.

    Unordered because `variant_match` is symmetric: the seed lists `bob,robert` and
    not its reverse, and a directed reading would let the generator emit `robert`
    for a `bob` persona and not the other way round.
    """
    with NICKNAME_SEED_PATH.open(encoding="utf-8", newline="") as handle:
        rows = list(csv.DictReader(handle))
    pairs: set[frozenset[str]] = set()
    for row in rows:
        variant_a = (row.get("variant_a") or "").strip()
        variant_b = (row.get("variant_b") or "").strip()
        if not variant_a or not variant_b:
            raise ValueError(f"{NICKNAME_SEED_PATH}: a row is missing one of its two variants")
        pairs.add(frozenset({variant_a, variant_b}))
    if not pairs:
        raise ValueError(f"{NICKNAME_SEED_PATH}: the seed lists no variant pairs")
    return frozenset(pairs)


@cache
def _variants_by_name() -> Mapping[str, tuple[str, ...]]:
    """name -> the other side of every seed pair it appears in, in sorted order.

    Sorted rather than file order: the draw indexes into this tuple, so its order is
    part of the corpus, and file order would make inserting a row into the seed move
    every substitution after it.
    """
    index: dict[str, set[str]] = {}
    for pair in nickname_pairs():
        first, second = sorted(pair)
        index.setdefault(first, set()).add(second)
        index.setdefault(second, set()).add(first)
    return MappingProxyType({name: tuple(sorted(others)) for name, others in index.items()})


def record_rng(seed: int, source: str, persona_index: int, ordinal: int) -> random.Random:
    """The RNG for one emitted record.

    Derived from the four coordinates rather than advanced from a single stream, so
    the corruption of one record does not depend on how many records were written
    before it. That is what lets `emit.py` write three files in one pass, and what
    keeps a `--batch` delivery reproducible without replaying the base corpus.

    SHA-256 rather than `hash()` or `random.Random(tuple)`: `hash()` of a `str` is
    salted by `PYTHONHASHSEED` and would make the corpus differ between two
    processes on the same machine, which is precisely what S10.1 forbids.
    """
    digest = hashlib.sha256(f"{seed}|{source}|{persona_index}|{ordinal}".encode()).digest()
    return random.Random(int.from_bytes(digest, "big"))


def render_phone(e164: str, form: str) -> str:
    """Render an E.164 US number in one of the three S8.2 surfaces.

    Args:
        e164: `+1` followed by ten digits, as `personas.py` draws it.
        form: a member of :data:`PHONE_FORMS`.

    Returns:
        The surface string. All three carry the same ten national digits, so
        `phone_e164` (S4.2) maps them back to one number and they block together.

    Raises:
        ValueError: ``e164`` is not a `+1`-prefixed eleven-digit number, or ``form``
            is not a known surface.
    """
    digits = e164.removeprefix("+")
    if not digits.isdigit() or len(digits) != 11 or not digits.startswith("1"):
        raise ValueError(f"expected a +1 E.164 US number, got {e164!r}")
    area, exchange, line = digits[1:4], digits[4:7], digits[7:]
    match form:
        case "e164":
            return e164
        case "dashed":
            return f"{area}-{exchange}-{line}"
        case "parens":
            return f"({area}) {exchange}-{line}"
        case _:
            raise ValueError(f"unknown phone form {form!r}; expected one of {PHONE_FORMS}")


def _apply_typo(text: str, rng: random.Random) -> str:
    """One character-level typo: adjacency substitution, transposition or deletion.

    The three are the shapes a keyed surname actually acquires. The value is
    returned unchanged when it is too short to survive one (:data:`MIN_TYPO_LENGTH`)
    -- the caller has already spent its trial, so a short name simply carries no
    typo rather than consuming a redraw and moving every later record.
    """
    if len(text) < MIN_TYPO_LENGTH:
        return text
    position = rng.randrange(len(text))
    kind = rng.randrange(3)
    if kind == 0:
        replacements = _ADJACENT.get(text[position])
        if not replacements:
            return text
        return (
            text[:position] + replacements[rng.randrange(len(replacements))] + text[position + 1 :]
        )
    if kind == 1:
        # Transposition needs a successor; at the last position, swap backwards.
        left = min(position, len(text) - 2)
        return text[:left] + text[left + 1] + text[left] + text[left + 2 :]
    return text[:position] + text[position + 1 :]


def _substitute_nickname(given_name: str, rng: random.Random) -> str:
    """The seed's counterpart of ``given_name``, or the name itself when it has none.

    The trial is the caller's; this only chooses which of several listed variants to
    use, so `james` can appear as `jim` or `jimmy` and both are justifiable against
    the seed.
    """
    variants = _variants_by_name().get(given_name)
    if not variants:
        return given_name
    return variants[rng.randrange(len(variants))]


def _stale_address_line(rng: random.Random) -> str:
    """A previous address in the same postal area, in the parser's spelling.

    Drawn from `personas.py`'s committed street vocabulary so the line stays inside
    the grammar `RegexV1Parser` (S4.2) accepts -- S10.1 admits only patterns the v1
    parser handles, and an address the pipeline cannot componentize is a corpus
    defect rather than a corruption. No unit phrase: the previous residence is a
    different building, and carrying the current unit designator over to it would be
    the one shape a reader would call a bug.
    """
    number = str(rng.randrange(1, MAX_HOUSE_NUMBER + 1))
    street = STREET_NAMES[rng.randrange(len(STREET_NAMES))]
    street_type = STREET_TYPES[rng.randrange(len(STREET_TYPES))]
    return f"{number} {street} {street_type}"


def _moved_address(rng: random.Random) -> tuple[str, str, str, str]:
    """`(line, city, region, postal)` for a person who left their postal area.

    Drawn from the same committed vocabulary as `personas.py`'s addresses so the
    line parses under the v1 grammar, and postal-distinct from the persona's own
    only probabilistically -- one draw in a thousand lands back in the same city
    block, which is corpus noise, not a defect. No unit phrase, for the stale
    axis's reason.
    """
    number = str(rng.randrange(1, MAX_HOUSE_NUMBER + 1))
    street = STREET_NAMES[rng.randrange(len(STREET_NAMES))]
    street_type = STREET_TYPES[rng.randrange(len(STREET_TYPES))]
    city, region, postal_base = CITIES[rng.randrange(len(CITIES))]
    postal = f"{postal_base + rng.randrange(POSTAL_SPREAD):05d}"
    return f"{number} {street} {street_type}", city, region, postal


def _alternate_given(rng: random.Random) -> str:
    """A different legal name from the committed weighted list.

    The draw may land on the persona's own name; the caller treats that as the
    trial having produced no visible drift, which keeps the axis one draw wide.
    """
    return given_names().pick(rng)


def _alternate_email(persona: Persona, rng: random.Random) -> str:
    """A second mailbox for the persona: their name plus a numeric disambiguator.

    The `.N` suffix keeps the value inside the corpus's `<given>.<family>@<domain>`
    idiom while making a collision with any persona's primary address (whose rare
    dedup suffix is digit-only, never dot-digit) impossible by construction.
    """
    domain = EMAIL_DOMAINS[rng.randrange(len(EMAIL_DOMAINS))]
    return f"{persona.given_name}.{persona.family_name}.{rng.randrange(2, 1000)}@{domain}"


def _swap_email_domain(email: str, rng: random.Random) -> str:
    """The same username on a different reserved domain -- `username_exact`'s shape."""
    local, _, current = email.rpartition("@")
    others = tuple(domain for domain in EMAIL_DOMAINS if domain != current)
    return f"{local}@{others[rng.randrange(len(others))]}"


def _alternate_phone(rng: random.Random) -> str:
    """A different fictional-exchange number -- an old or secondary line."""
    area = AREA_CODES[rng.randrange(len(AREA_CODES))]
    return f"+1{area}{FICTIONAL_EXCHANGE}{rng.randrange(10_000):04d}"


def _dob_day_typo(birth_date: date, rng: random.Random) -> date:
    """The same year and month, a different day -- `dob_same_year_month`'s shape."""
    month_start = birth_date.replace(day=1)
    next_month = (month_start + timedelta(days=31)).replace(day=1)
    days_in_month = (next_month - month_start).days
    offset = rng.randrange(1, days_in_month)
    return birth_date.replace(day=1 + (birth_date.day - 1 + offset) % days_in_month)


def _wrong_birth_date(rng: random.Random) -> date:
    """A uniform redraw over the persona window -- almost surely a different month."""
    return date.fromordinal(
        rng.randrange(EARLIEST_BIRTH_DATE.toordinal(), LATEST_BIRTH_DATE.toordinal() + 1)
    )


def _titlecase(text: str) -> str:
    """Capitalise word-initially, leaving digit- and `#`-initial tokens alone.

    `str.title()` renders `221b` as `221B` and `3rd` as `3Rd`; both survive
    `address_parse`'s lowercasing, but a committed corpus a human reads during a
    benchmark post-mortem should not look machine-mangled. Casing is itself S10.1
    format drift: `name_norm` (S4.2) lowercases, so the pipeline is required to be
    indifferent to it and the corpus is where that gets exercised.
    """
    return " ".join(
        word if word[:1].isdigit() or word.startswith("#") else word.capitalize()
        for word in text.split(" ")
    )


def corrupt_record(
    persona: Persona, profile: CorruptionProfile, rng: random.Random
) -> CorruptedRecord:
    """Apply ``profile`` to ``persona``, returning what its source system holds.

    Args:
        persona: the ground truth, from `personas.py`.
        profile: the source's corruption rates.
        rng: this record's stream, from :func:`record_rng`.

    Returns:
        The record's surface values. `persona_id` travels with them (S10.1, M21) so
        the emitted row carries its own truth label.

    Note:
        The axes are applied in a fixed order and each spends exactly one trial,
        fired or not, so adding a rate to a profile moves only the axes after it.
        Where two axes drift the same field, the trial of each is still spent and
        the first-listed one wins, so the pair stays two independent Bernoulli
        draws rather than one axis reshaping the other's realised rate.
    """
    nicknamed = profile.fires(profile.nickname_rate, rng)
    given_name = _substitute_nickname(persona.given_name, rng) if nicknamed else persona.given_name
    if profile.fires(profile.alt_given_rate, rng) and not nicknamed:
        given_name = _alternate_given(rng)

    family_name = persona.family_name
    if profile.fires(profile.typo_rate, rng):
        family_name = _apply_typo(family_name, rng)

    address_line = persona.address_line
    addr_city = persona.addr_city
    addr_region = persona.addr_region
    addr_postal = persona.addr_postal
    moved = profile.fires(profile.moved_postal_rate, rng)
    if moved:
        address_line, addr_city, addr_region, addr_postal = _moved_address(rng)
    if profile.fires(profile.stale_address_rate, rng) and not moved:
        address_line = _stale_address_line(rng)

    number = _alternate_phone(rng) if profile.fires(profile.alt_phone_rate, rng) else persona.phone
    phone = render_phone(number, profile.pick_phone_form(rng))

    email = persona.email
    alt_email = profile.fires(profile.alt_email_rate, rng)
    if alt_email:
        email = _alternate_email(persona, rng)
    if profile.fires(profile.email_domain_swap_rate, rng) and not alt_email:
        email = _swap_email_domain(email, rng)

    birth_date: date | None = persona.birth_date
    day_typo = profile.fires(profile.dob_day_typo_rate, rng)
    if day_typo:
        birth_date = _dob_day_typo(persona.birth_date, rng)
    if profile.fires(profile.dob_wrong_rate, rng) and not day_typo:
        birth_date = _wrong_birth_date(rng)

    if profile.fires(profile.missing_rate("email"), rng):
        email = ""
    if profile.fires(profile.missing_rate("phone"), rng):
        phone = ""
    if profile.fires(profile.missing_rate("birth_date"), rng):
        birth_date = None

    return CorruptedRecord(
        persona_id=persona.persona_id,
        given_name=_titlecase(given_name),
        family_name=_titlecase(family_name),
        email=email,
        phone=phone,
        address_line=_titlecase(address_line),
        addr_city=_titlecase(addr_city),
        addr_region=addr_region.upper(),
        addr_postal=addr_postal,
        birth_date=birth_date,
    )


def _rate(source: str, field: str, raw: Any) -> float:
    """One rate, validated as a real number inside ``[0, 1]``."""
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        raise ValueError(f"profiles.{source}.{field}: expected a number, got {raw!r}")
    value = float(raw)
    if not 0.0 <= value <= 1.0:
        raise ValueError(f"profiles.{source}.{field}: rate must be within [0, 1], got {value}")
    return value


def _phone_form_weights(source: str, raw: Any) -> Mapping[str, int]:
    """The phone-surface weights, validated as positive integers over known forms."""
    if not isinstance(raw, dict):
        raise ValueError(f"profiles.{source}.phone_form_weights: expected a mapping, got {raw!r}")
    weights: dict[str, int] = {}
    for form, weight in raw.items():
        if form not in PHONE_FORMS:
            raise ValueError(
                f"profiles.{source}.phone_form_weights: unknown form {form!r}; "
                f"expected one of {PHONE_FORMS}"
            )
        if isinstance(weight, bool) or not isinstance(weight, int) or weight <= 0:
            raise ValueError(
                f"profiles.{source}.phone_form_weights.{form}: expected a positive integer, "
                f"got {weight!r}"
            )
        weights[form] = weight
    if not weights:
        raise ValueError(f"profiles.{source}.phone_form_weights: no surface is enabled")
    return MappingProxyType(weights)


def _missing_rates(source: str, raw: Any) -> Mapping[str, float]:
    """The per-field missingness, validated against :data:`MISSABLE_FIELDS`."""
    if raw is None:
        return MappingProxyType({})
    if not isinstance(raw, dict):
        raise ValueError(f"profiles.{source}.missing_rates: expected a mapping, got {raw!r}")
    rates: dict[str, float] = {}
    for field, value in raw.items():
        if field not in MISSABLE_FIELDS:
            raise ValueError(
                f"profiles.{source}.missing_rates: {field!r} may not go missing; "
                f"expected one of {MISSABLE_FIELDS}"
            )
        rates[field] = _rate(source, f"missing_rates.{field}", value)
    return MappingProxyType(rates)


def load_profiles(
    config: Config, path: Path | str | None = None
) -> Mapping[str, CorruptionProfile]:
    """Read and validate `profiles.yaml` against the tenant config.

    Args:
        config: the validated S6 document. It owns the source vocabulary, so a
            profile is checked against it rather than against a list restated here.
        path: the profile document; :data:`PROFILES_PATH` when omitted.

    Returns:
        One profile per configured source, keyed by source name.

    Raises:
        ValueError: the document is not a mapping of sources to rate blocks, names a
            source absent from `sources:` in ``config``, omits a configured source,
            carries an unknown key, or states a rate outside ``[0, 1]``.

    Note:
        Both directions are errors on purpose. An unknown source is a typo that
        would otherwise corrupt nothing and be invisible; a *missing* one would emit
        a whole source with no drift at all, which reads in the S10.5 quality
        numbers as an unusually clean corpus rather than as a broken profile.
    """
    document_path = Path(path) if path is not None else PROFILES_PATH
    with document_path.open(encoding="utf-8") as handle:
        document = yaml.safe_load(handle)
    if not isinstance(document, dict):
        raise ValueError(f"{document_path}: expected a mapping of source name to profile")

    unknown = sorted(set(document) - set(config.sources))
    if unknown:
        raise ValueError(
            f"{document_path}: profiles name source(s) {unknown} absent from sources: in the config"
        )
    absent = sorted(set(config.sources) - set(document))
    if absent:
        raise ValueError(f"{document_path}: configured source(s) {absent} have no profile")

    profiles: dict[str, CorruptionProfile] = {}
    for source, block in document.items():
        if not isinstance(block, dict):
            raise ValueError(f"profiles.{source}: expected a mapping, got {block!r}")
        known = {*_RATE_FIELDS, "missing_rates", "phone_form_weights"}
        extra = sorted(set(block) - known)
        if extra:
            raise ValueError(f"profiles.{source}: unknown key(s) {extra}; expected {sorted(known)}")
        missing_keys = sorted(known - set(block))
        if missing_keys:
            raise ValueError(f"profiles.{source}: missing key(s) {missing_keys}")
        rates = {field: _rate(source, field, block[field]) for field in _RATE_FIELDS}
        profiles[source] = CorruptionProfile(
            source=source,
            **rates,
            missing_rates=_missing_rates(source, block["missing_rates"]),
            phone_form_weights=_phone_form_weights(source, block["phone_form_weights"]),
        )
    return MappingProxyType(profiles)
