"""Versioned adversarial errors, independent of baseline generation's RNG stream.

Rates apply per record except the shared-contact household axis (per persona pair).
Truth identity is never changed. This profile deliberately includes unrecoverable
cases so retrieval recall is measured rather than guaranteed by construction.
"""

from dataclasses import replace

from .corruptions import CorruptedRecord, record_rng
from .personas import Persona

PROFILE = "hard-v1"
RATES = {
    "given_typo": 0.05,
    "surname_prefix_typo": 0.05,
    "unseen_nickname": 0.10,
    "swapped_names": 0.02,
    "missing_names": 0.02,
    "missing_address": 0.03,
    "moved_and_changed_contacts": 0.05,
    "shared_contact_household": 0.02,
}
NICKNAMES = {
    "elizabeth": "libby",
    "alexander": "sasha",
    "margaret": "greta",
    "william": "wim",
    "james": "jem",
    "charles": "chaz",
    "susan": "suki",
}


def corrupt_hard(
    record: CorruptedRecord,
    *,
    seed: int,
    source: str,
    index: int,
    ordinal: int,
    partner: Persona,
    household_anchor: Persona,
) -> CorruptedRecord:
    """Apply independent errors and correlated moves; preserve deterministic truth."""
    rng = record_rng(seed, source + ":" + PROFILE, index, ordinal)

    def fires(axis: str) -> bool:
        return rng.randrange(10_000) < round(RATES[axis] * 10_000)

    given, family = record.given_name, record.family_name
    if fires("unseen_nickname"):
        given = NICKNAMES.get(given.lower(), given)
    if fires("given_typo") and len(given) >= 2:
        given = given[1] + given[0] + given[2:]
    if fires("surname_prefix_typo") and family:
        family = ("x" if family[0].lower() != "x" else "z") + family[1:]
    if fires("swapped_names"):
        given, family = family, given
    if fires("missing_names"):
        given, family = "", ""
    result = replace(record, given_name=given, family_name=family)
    if fires("moved_and_changed_contacts"):
        # A real different persona's contact/address values create hard negatives
        # as well as correlated drift, while birth date remains an independent clue.
        result = replace(
            result,
            email=partner.email,
            phone=partner.phone,
            address_line=partner.address_line,
            addr_city=partner.addr_city,
            addr_region=partner.addr_region,
            addr_postal=partner.addr_postal,
        )
    if fires("missing_address"):
        result = replace(result, address_line="", addr_city="", addr_region="", addr_postal="")
    shared = record_rng(seed, PROFILE + ":household", index // 2, 0)
    if shared.randrange(10_000) < round(RATES["shared_contact_household"] * 10_000):
        # Both people in a pair share contacts, surname and address on this source.
        # The even person's stable anchor is supplied for both records.
        result = replace(
            result,
            email=household_anchor.email,
            phone=household_anchor.phone,
            family_name=household_anchor.family_name,
            address_line=household_anchor.address_line,
            addr_city=household_anchor.addr_city,
            addr_region=household_anchor.addr_region,
            addr_postal=household_anchor.addr_postal,
        )
    return result
