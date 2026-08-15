"""Ground-truth persons for the seeded synthetic corpus (DesignDoc.md S10.1, S10.2).

`generate_personas(seed, n, household_rate)` is the first half of the generator: the
people, before any source ever sees them. `corruptions.py` and `emit.py` (ER-051)
turn one persona into the several drifted records a source system would hold; every
one of those records carries the `persona_id` allocated here, and that label is what
makes the benchmark report match quality beside speed (S10.1) and what T-TRAIN-1
compares a regenerated model against (S8.3).

Four properties are load-bearing, and each one is a decision rather than an accident:

* **The arguments are the only entropy.** The function is a pure function of
  `(seed, n, household_rate)` -- no clock, no `random.*` module call, no `hash()`,
  no dependence on `set` or `dict` iteration order. Determinism has to hold ACROSS
  PROCESSES, not merely within one, because T-TRAIN-1 asserts byte equality between
  a committed model and one retrained in a different process on a different machine
  (S8.3), and the corpus is that model's input. The single `random.Random(seed)`
  instance is drawn from through `randrange` and `shuffle` only, and every weighted
  draw is integer arithmetic over integer weights: a float cumulative weight would
  make the corpus depend on summation order and rounding.
* **Names come from committed weighted lists.** S10.1 wants "realistic skew" so the
  S4.3.3 term-frequency adjustment has something to adjust. `data/given_names.csv`
  and `data/family_names.csv` are those lists, and their provenance is written into
  their own header rather than inferred from the shape of the draw.
* **A household shares an address and nothing else.** S10.1's household rate
  reproduces `base_10`'s shared-household trap at scale: several people at one
  address who MUST NOT merge. Members therefore share all six `addr_*` components
  exactly and share no `email`, `phone`, `family_name` or `birth_date` -- if they
  shared any of those, the pair would be a genuine duplicate and the "must not
  merge" guarantee would be untestable rather than merely unmet. Emails and phones
  are unique across the whole draw for the same reason: no cross-persona link may be
  designed in by accident.
* **Only addresses the v1 parser handles are emitted.** S4.2's `RegexV1Parser` is
  fixed; the generator adapts to it, never the reverse. The vocabulary below is
  chosen so `parse(address_line)` returns exactly the components the persona
  carries, which is why every value here is already in the parser's normalized
  spelling -- lowercase, no punctuation. Surface variation (casing, `.`/`,`,
  abbreviation drift) is a corruption, and corruptions are ER-051's job.

The module reads the two weight lists and performs no other I/O.
"""

from __future__ import annotations

import csv
import random
from bisect import bisect_right
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import date
from functools import cache
from itertools import accumulate
from pathlib import Path
from typing import Final

__all__ = [
    "FAMILY_NAMES_PATH",
    "GIVEN_NAMES_PATH",
    "HOUSEHOLD_ID_PREFIX",
    "PERSONA_ID_PREFIX",
    "PERSONA_ID_WIDTH",
    "Persona",
    "WeightedNames",
    "family_names",
    "generate_personas",
    "given_names",
]

DATA_DIR: Final = Path(__file__).resolve().parent / "data"
GIVEN_NAMES_PATH: Final = DATA_DIR / "given_names.csv"
FAMILY_NAMES_PATH: Final = DATA_DIR / "family_names.csv"

#: `P` followed by this many digits. The width is a CONSTANT, not derived from `n`:
#: persona 7 is `P00000007` at every scale, so a `truth.csv` written for the `smoke`
#: corpus cannot be read against the `10k` one and silently mislabel it (S10.2).
PERSONA_ID_WIDTH: Final = 8
PERSONA_ID_PREFIX: Final = "P"
HOUSEHOLD_ID_PREFIX: Final = "H"

#: How many times a draw may be rejected before we call the vocabulary too small for
#: the requested `n`. Bounded rather than `while True` so an exhausted space is an
#: error naming what ran out, not a benchmark run that hangs.
MAX_REDRAWS: Final = 10_000

#: Household sizes, sampled uniformly from this tuple, so two thirds of households
#: are pairs. Sizes above three would need a fourth distinct family name and DOB per
#: address for no extra coverage: the trap is "different people, same address", and
#: three people already exercise every pair of it.
HOUSEHOLD_SIZES: Final = (2, 2, 3)

#: Written as one whitespace-separated vocabulary rather than fifty quoted lines:
#: the list is data, and the shape of the literal should not dominate the module.
#: The four ordinals at the end are deliberate -- the parser's house-number rule
#: stops at the first token that is digits plus at most one letter followed by a
#: boundary, so `5th` is street and never number, and including a few keeps that
#: boundary exercised by the corpus and not only by the parser's own unit tests.
STREET_NAMES: Final = tuple(
    "maple oak cedar pine elm willow birch walnut chestnut spruce sycamore aspen magnolia "
    "juniper laurel poplar hickory cypress dogwood alder evergreen meadow sunset sunrise "
    "lakeview hillcrest riverside brookside parkside highland fairview woodland prospect "
    "summit valley harbor bayberry clover orchard garden franklin jefferson lincoln "
    "madison monroe quincy 3rd 5th 12th 42nd".split()
)

#: No entry may be a unit keyword (`apt`, `unit`, `ste`, `fl`, `rm`, `room`,
#: `floor`, `suite`, `apartment`): the parser's unit phrase is end-anchored, so a
#: street type spelled like one would be stripped off as a unit and the address
#: would not round-trip.
STREET_TYPES: Final = ("st", "ave", "rd", "dr", "ln", "blvd", "ct", "pl", "ter", "way")

#: Every unit spelling the v1 grammar recognises, `#` form included.
UNIT_FORMS: Final = ("apt {0}", "unit {0}", "ste {0}", "rm {0}", "fl {0}", "#{0}")
UNIT_LETTERS: Final = ("a", "b", "c", "d", "e", "f")
#: One address in this many carries a unit, and one designator in three has a letter.
UNIT_ODDS: Final = 4
UNIT_LETTER_ODDS: Final = 3

MAX_HOUSE_NUMBER: Final = 9_999
#: `221b` is a house number the grammar accepts; one address in this many uses one,
#: which is enough for the shape to appear in a 4,000-persona draw without making it
#: the common case.
NUMBER_SUFFIX_ODDS: Final = 40
NUMBER_SUFFIXES: Final = ("a", "b", "c")

#: `(city, region, postal_base)`. The postal code is `postal_base` plus an offset
#: below, rendered five digits wide so a leading zero survives as a string -- Boston
#: is `02108`, and an integer postal column would have made it `2108` in every
#: comparison and blocking key that touches it (S5).
CITIES: Final = (
    ("seattle", "wa", 98101),
    ("portland", "or", 97201),
    ("denver", "co", 80202),
    ("austin", "tx", 78701),
    ("boston", "ma", 2108),
    ("chicago", "il", 60601),
    ("phoenix", "az", 85001),
    ("atlanta", "ga", 30301),
    ("miami", "fl", 33101),
    ("detroit", "mi", 48201),
    ("houston", "tx", 77001),
    ("nashville", "tn", 37201),
    ("columbus", "oh", 43201),
    ("raleigh", "nc", 27601),
    ("madison", "wi", 53701),
    ("omaha", "ne", 68101),
    ("tucson", "az", 85701),
    ("fresno", "ca", 93701),
    ("mesa", "az", 85201),
    ("tampa", "fl", 33601),
)
POSTAL_SPREAD: Final = 50

#: RFC 2606 reserved domains only. A generated address must not be deliverable to a
#: real mailbox, and `example.*` is the one guarantee of that which needs no upkeep.
#: None of these can collide with `standardization.email_placeholders` (S6) either,
#: since every local part here is `<given>.<family>`.
EMAIL_DOMAINS: Final = (
    "example.com",
    "example.net",
    "example.org",
    "mail.example.com",
    "inbox.example.net",
)

#: The exchange reserved for fiction. Every generated number is
#: `+1<area code>555<line>`, so no persona can be given a real subscriber's line,
#: and `phone_e164` (S4.2) still renders it: that macro is digit-length logic, not a
#: carrier lookup, so a 555 number is `phone_valid` exactly like any other.
FICTIONAL_EXCHANGE: Final = "555"
#: N11 codes (211 ... 911) are service codes and are never assigned as area codes.
AREA_CODES: Final = tuple(code for code in range(200, 1000) if code % 100 != 11)

#: The DOB window, as a full date range rather than a year: S4.2's `parse_date`
#: NULLs a year-only value, so a year-precision persona would be ground truth the
#: pipeline is required to throw away.
EARLIEST_BIRTH_DATE: Final = date(1935, 1, 1)
LATEST_BIRTH_DATE: Final = date(2006, 12, 31)
_EARLIEST_BIRTH_ORDINAL: Final = EARLIEST_BIRTH_DATE.toordinal()
_LATEST_BIRTH_ORDINAL: Final = LATEST_BIRTH_DATE.toordinal()


@dataclass(frozen=True, slots=True)
class WeightedNames:
    """A committed name list with its integer weights and their prefix sums."""

    names: tuple[str, ...]
    weights: tuple[int, ...]
    #: Inclusive prefix sums, so a draw is one `randrange` and one binary search.
    #: Integers throughout: floats would make the same seed produce a different
    #: name on a machine that sums them differently.
    cumulative: tuple[int, ...]

    @property
    def total(self) -> int:
        return self.cumulative[-1]

    def share(self, name: str) -> float:
        """The committed relative frequency of ``name``; ``0.0`` if unlisted."""
        for candidate, weight in zip(self.names, self.weights, strict=True):
            if candidate == name:
                return weight / self.total
        return 0.0

    def pick(self, rng: random.Random) -> str:
        """One weighted draw."""
        return self.names[bisect_right(self.cumulative, rng.randrange(self.total))]


@dataclass(frozen=True, slots=True)
class Persona:
    """One ground-truth person: the entity every record labelled with it describes.

    Frozen because a persona is the truth a run is graded against; a caller that
    could edit one after the fact could make a failing benchmark pass.

    The six `addr_*` fields and `address_line` are both present and are not
    redundant: S6 feeds `address_line` to the parser and maps the other three
    directly, so a source row needs the single line while the ground truth needs the
    components the pipeline is expected to recover from it.
    """

    persona_id: str
    given_name: str
    family_name: str
    email: str
    phone: str
    address_line: str
    addr_number: str
    addr_street: str
    addr_unit: str | None
    addr_city: str
    addr_region: str
    addr_postal: str
    birth_date: date
    household_id: str


@dataclass(frozen=True, slots=True)
class _Address:
    """The six components of one address, and the single line they render to."""

    number: str
    street: str
    unit: str | None
    city: str
    region: str
    postal: str

    @property
    def line(self) -> str:
        """The `address_line` a source system would hold.

        A missing unit contributes nothing rather than an empty token, which is what
        makes `RegexV1Parser.parse(line)` return these same components back.
        """
        return " ".join(part for part in (self.number, self.street, self.unit) if part)


@cache
def _load_weighted_names(path: Path) -> WeightedNames:
    """Read one `name,weight` list, skipping its `#` provenance header.

    Cached: the lists are committed data and cannot change under a running process,
    and re-reading them per call would make a 400,000-persona draw pay for the file
    once per persona.
    """
    with path.open(encoding="utf-8", newline="") as handle:
        rows = [line for line in handle if not line.startswith("#")]

    names: list[str] = []
    weights: list[int] = []
    for row in csv.DictReader(rows):
        name = (row.get("name") or "").strip()
        raw_weight = (row.get("weight") or "").strip()
        if not name:
            raise ValueError(f"{path}: a row carries no name")
        try:
            weight = int(raw_weight)
        except ValueError:
            raise ValueError(f"{path}: {name!r} has a non-integer weight {raw_weight!r}") from None
        if weight <= 0:
            raise ValueError(f"{path}: {name!r} has a non-positive weight {weight}")
        names.append(name)
        weights.append(weight)

    if len(names) < 2:
        raise ValueError(f"{path}: a weighted list needs at least two names")
    if len(set(names)) != len(names):
        raise ValueError(f"{path}: the list repeats a name; a weight must be stated once")
    return WeightedNames(tuple(names), tuple(weights), tuple(accumulate(weights)))


def given_names() -> WeightedNames:
    """The committed given-name list."""
    return _load_weighted_names(GIVEN_NAMES_PATH)


def family_names() -> WeightedNames:
    """The committed family-name list."""
    return _load_weighted_names(FAMILY_NAMES_PATH)


def _pick[ItemT](items: Sequence[ItemT], rng: random.Random) -> ItemT:
    """Uniform choice through `randrange`.

    `Random.choice` would do the same thing today; spelling it out keeps every draw
    in this module on the two primitives -- `randrange` and `shuffle` -- whose
    behaviour the corpus's byte-reproducibility depends on.
    """
    return items[rng.randrange(len(items))]


def _draw_unique[ValueT](draw: Callable[[], ValueT], taken: set[ValueT], what: str) -> ValueT:
    """Draw until the value is new to ``taken``, then record it."""
    for _ in range(MAX_REDRAWS):
        value = draw()
        if value not in taken:
            taken.add(value)
            return value
    raise RuntimeError(f"no unused {what} after {MAX_REDRAWS} draws; the vocabulary is too small")


def _draw_birth_date(rng: random.Random) -> date:
    """A uniform full-precision date in the window.

    Drawn as an ordinal rather than as `(year, month, day)`: a per-component draw
    either has to reject the 31st of February or clamp the day to 28, and clamping
    would leave the corpus with no birthdays after the 28th at all -- a shape no
    real source has and one a `dob_same_year_month` comparison level would see.
    """
    return date.fromordinal(rng.randrange(_EARLIEST_BIRTH_ORDINAL, _LATEST_BIRTH_ORDINAL + 1))


def _draw_phone(rng: random.Random) -> str:
    """An E.164 number in the fictional exchange."""
    return f"+1{_pick(AREA_CODES, rng)}{FICTIONAL_EXCHANGE}{rng.randrange(10_000):04d}"


def _draw_address(rng: random.Random) -> _Address:
    """One address, in the parser's own normalized spelling."""
    number = str(rng.randrange(1, MAX_HOUSE_NUMBER + 1))
    if rng.randrange(NUMBER_SUFFIX_ODDS) == 0:
        number += _pick(NUMBER_SUFFIXES, rng)

    street = f"{_pick(STREET_NAMES, rng)} {_pick(STREET_TYPES, rng)}"

    unit: str | None = None
    if rng.randrange(UNIT_ODDS) == 0:
        designator = str(rng.randrange(1, 400))
        if rng.randrange(UNIT_LETTER_ODDS) == 0:
            designator += _pick(UNIT_LETTERS, rng)
        unit = _pick(UNIT_FORMS, rng).format(designator)

    city, region, postal_base = _pick(CITIES, rng)
    return _Address(
        number=number,
        street=street,
        unit=unit,
        city=city,
        region=region,
        postal=f"{postal_base + rng.randrange(POSTAL_SPREAD):05d}",
    )


def _draw_email(rng: random.Random, given_name: str, family_name: str, taken: set[str]) -> str:
    """`<given>.<family>@<domain>`, disambiguated by a counter when it collides.

    The counter rather than a redraw is deliberate: it consumes exactly one value
    from the RNG whether or not the name pair has been seen, so adding a name to a
    weight list moves the names that follow it and nothing else.
    """
    domain = _pick(EMAIL_DOMAINS, rng)
    local = f"{given_name}.{family_name}"
    candidate = f"{local}@{domain}"
    suffix = 1
    while candidate in taken:
        suffix += 1
        candidate = f"{local}{suffix}@{domain}"
    taken.add(candidate)
    return candidate


def _households(rng: random.Random, n: int, household_rate: float) -> list[tuple[int, ...]]:
    """Partition persona indices into households, ordered by their first member.

    Every index appears exactly once, and `round(n * household_rate)` of them land
    in a household of two or three. The count is exact rather than a per-persona
    Bernoulli trial: S10.1 makes the rate an input to the corpus, and a realised
    share that wanders two points either side of it at n = 4,000 would make the
    shared-address trap density a property of the seed instead of of the argument.
    """
    order = list(range(n))
    rng.shuffle(order)
    shared_target = round(n * household_rate)
    pool = order[:shared_target] if shared_target >= 2 else []

    groups: list[tuple[int, ...]] = []
    while len(pool) >= 2:
        size = _pick(HOUSEHOLD_SIZES, rng)
        if len(pool) - size == 1:
            # Never strand one person: they would fall out of every household and
            # the realised share would sit below the requested rate.
            size = len(pool) - 2 if len(pool) - 2 >= 2 else len(pool)
        size = min(size, len(pool))
        groups.append(tuple(sorted(pool[:size])))
        pool = pool[size:]

    housed = {index for group in groups for index in group}
    groups.extend((index,) for index in range(n) if index not in housed)
    # Sorted by first member, so the household a persona belongs to -- and therefore
    # the order the draws happen in -- does not depend on `rng.shuffle`'s output
    # order surviving into an iteration order anywhere.
    groups.sort(key=lambda group: group[0])
    return groups


def generate_personas(seed: int, n: int, household_rate: float) -> list[Persona]:
    """Return ``n`` ground-truth personas, determined entirely by the arguments.

    Args:
        seed: `generator.seed` from the tenant config (S6). A literal here rather
            than a config value would be M26's mistake, so callers pass it in.
        n: how many personas; the `Personas` column of the S10.2 scale table.
        household_rate: the fraction of personas that share an address with at least
            one other persona.

    Returns:
        The personas in `persona_id` order.

    Raises:
        ValueError: ``n`` is negative or ``household_rate`` is outside ``[0, 1]``.
        RuntimeError: the committed vocabulary cannot supply ``n`` distinct
            addresses, family names or phone numbers.
    """
    if n < 0:
        raise ValueError(f"n must be >= 0, got {n}")
    if not 0.0 <= household_rate <= 1.0:
        raise ValueError(f"household_rate must be within [0, 1], got {household_rate}")

    rng = random.Random(seed)
    given = given_names()
    family = family_names()

    seen_emails: set[str] = set()
    seen_phones: set[str] = set()
    seen_addresses: set[_Address] = set()

    drafts: list[tuple[int, Persona]] = []
    for house_number, members in enumerate(_households(rng, n, household_rate)):
        household_id = f"{HOUSEHOLD_ID_PREFIX}{house_number:0{PERSONA_ID_WIDTH}d}"
        address = _draw_unique(lambda: _draw_address(rng), seen_addresses, "address")
        # Per household, not per corpus: S10.1 wants people who share an address and
        # nothing else, so these two are what a household member may NOT repeat.
        used_family_names: set[str] = set()
        used_birth_dates: set[date] = set()

        for index in members:
            family_name = _draw_unique(lambda: family.pick(rng), used_family_names, "family name")
            birth_date = _draw_unique(lambda: _draw_birth_date(rng), used_birth_dates, "birth date")
            given_name = given.pick(rng)
            drafts.append(
                (
                    index,
                    Persona(
                        persona_id=f"{PERSONA_ID_PREFIX}{index:0{PERSONA_ID_WIDTH}d}",
                        given_name=given_name,
                        family_name=family_name,
                        email=_draw_email(rng, given_name, family_name, seen_emails),
                        phone=_draw_unique(lambda: _draw_phone(rng), seen_phones, "phone number"),
                        address_line=address.line,
                        addr_number=address.number,
                        addr_street=address.street,
                        addr_unit=address.unit,
                        addr_city=address.city,
                        addr_region=address.region,
                        addr_postal=address.postal,
                        birth_date=birth_date,
                        household_id=household_id,
                    ),
                )
            )

    return [persona for _, persona in sorted(drafts, key=lambda draft: draft[0])]
