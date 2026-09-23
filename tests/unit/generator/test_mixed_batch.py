"""The mixed delivery adds record keys, with explicit existing/new-person truth."""

from __future__ import annotations

import csv
import sys
from collections import Counter
from pathlib import Path

from er.config.loader import load_config


def generator_module(name: str):
    sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "fixtures"))
    return __import__(f"generator.{name}", fromlist=["_"])


emit = generator_module("emit")
generate_personas = generator_module("personas").generate_personas


def truth(path: Path) -> list[dict[str, str]]:
    with path.open() as handle:
        return list(csv.DictReader(handle))


def test_mixed_batch_keeps_base_bytes_and_has_disjoint_keys(tmp_path: Path) -> None:
    config = load_config(Path("configs/default.yaml"))
    people = generate_personas(42, 60, 0.1)
    for scenario in ("existing", "mixed-v1"):
        spec = emit.CorpusSpec(
            seed=42, personas=60, records=150, batch=100, incremental_scenario=scenario
        )
        emit.emit_corpus(spec, people, tmp_path / scenario, config=config)
    for source in ("crm", "billing", "webforms", "truth"):
        assert (tmp_path / "existing" / f"{source}.csv").read_bytes() == (
            tmp_path / "mixed-v1" / f"{source}.csv"
        ).read_bytes()
    base = truth(tmp_path / "mixed-v1/truth.csv")
    batch = truth(tmp_path / "mixed-v1/batch/truth.csv")
    base_people = {row["persona_id"] for row in base}
    old_counts = Counter(row["persona_id"] for row in batch if row["persona_id"] in base_people)
    new_counts = Counter(row["persona_id"] for row in batch if row["persona_id"] not in base_people)
    assert len(old_counts) == sum(old_counts.values()) == 50
    assert len(new_counts) == 20
    assert sum(new_counts.values()) == 50
    assert set(new_counts.values()) == {2, 3}

    def keys(rows):
        return {(row["source_system"], row["source_record_id"]) for row in rows}

    assert not keys(base) & keys(batch)
    assert len(keys(batch)) == 100


def test_new_people_cannot_collide_with_existing_contact_keys() -> None:
    base = generate_personas(42, 100, 0.2)
    added = generate_personas(43, 20, 0.2, existing=base)
    assert added == generate_personas(43, 20, 0.2, existing=base)
    for field in ("persona_id", "email", "phone", "household_id"):
        assert not {getattr(person, field) for person in base} & {
            getattr(person, field) for person in added
        }
