"""Regenerate the er-std lexicon golden oracle from the Python er.std.lexicon.

    uv run python rust/crates/er-std/tests/gen_lexicon_golden.py

Covers canonical_pair (normalize=strip+lower, reject empty/equal, orient min<max)
and lexicon_hash over an active pair set (orient + sorted/deduped set, join each
pair with \\x1e and pairs with \\x1f, sha256 hex; empty set = sha256 of "").
"""

import hashlib
import json
from pathlib import Path

from er.errors import ConfigError
from er.std.lexicon import _canonical, packaged_seed_pairs

HERE = Path(__file__).resolve().parent
UNIT = "\x1e"
REC = "\x1f"


def lexhash(pairs):
    s = sorted({(a, b) if a < b else (b, a) for a, b in pairs})
    joined = REC.join(f"{a}{UNIT}{b}" for a, b in s)
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()


def main() -> None:
    packaged = [list(p) for p in packaged_seed_pairs()]
    orient = [["bob", "robert"], ["robert", "bob"], ["al", "albert"]]
    hash_cases = [
        {"name": "empty", "pairs": [], "hash": lexhash([])},
        {"name": "packaged", "pairs": packaged, "hash": lexhash([tuple(p) for p in packaged])},
        {"name": "orient_dups", "pairs": orient, "hash": lexhash([tuple(p) for p in orient])},
    ]

    canon_cases = []
    for a, b in [("Bob", " Robert "), ("AL", "albert"), ("tony", "Anthony"),
                 ("Al", "al"), ("", "x"), ("  ", "y"), ("Bob", "bob")]:
        try:
            x, y = _canonical(a, b)
            canon_cases.append({"a": a, "b": b, "ok": True, "out": [x, y]})
        except ConfigError:
            canon_cases.append({"a": a, "b": b, "ok": False})

    doc = {
        "_comment": "Golden er.std.lexicon oracle. Regenerate: uv run python rust/crates/er-std/tests/gen_lexicon_golden.py",
        "hash_cases": hash_cases,
        "canon_cases": canon_cases,
    }
    (HERE / "lexicon_golden.json").write_text(
        json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(hash_cases)} hash + {len(canon_cases)} canon cases")


if __name__ == "__main__":
    main()
