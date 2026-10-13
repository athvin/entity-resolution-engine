"""Regenerate the er-config golden oracle from the real Python er.config.

Run from the repo root with the Python project's env:

    uv run python rust/crates/er-config/tests/gen_golden.py

It loads the committed configs/*.yaml, computes the S5.2 canonical document and
config_hash the exact way src/er/config/hashing.py does, and writes
config_golden.json next to this script. The Rust test (tests/config_hash.rs)
parses the same vendored YAML and asserts byte-identical output.
"""

import json
from pathlib import Path

from er.config.hashing import canonical_document, config_hash
from er.config.loader import load_config

HERE = Path(__file__).resolve().parent
CONFIGS = [("default", "configs/default.yaml"), ("test", "configs/test.yaml")]


def main() -> None:
    cases = []
    for name, path in CONFIGS:
        cfg = load_config(path)
        cases.append(
            {
                "name": name,
                "yaml": f"{name}.yaml",
                "canonical_document": canonical_document(cfg),
                "config_hash": config_hash(cfg),
            }
        )
    doc = {
        "_comment": (
            "Golden config_hash oracle from the Python er.config over configs/*.yaml. "
            "Regenerate: uv run python rust/crates/er-config/tests/gen_golden.py"
        ),
        "cases": cases,
    }
    (HERE / "config_golden.json").write_text(
        json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote golden with {len(cases)} cases")


if __name__ == "__main__":
    main()
