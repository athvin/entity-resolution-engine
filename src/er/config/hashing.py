"""``config_hash``, defined normatively in DesignDoc.md S5.2.

S5.2: the config is loaded, validated into the Pydantic model, dumped with keys
sorted, no aliases, UTF-8, compact separators, and hashed with SHA-256; the hex
digest is ``config_hash``. It is written to ``runs.config_hash``,
``model_registry.config_hash`` and every stage's log line, and it is not itself a
config field (S6).

Hashing the validated document rather than the YAML text is what makes the digest
insensitive to comments, key order and block style, and — because the loader
normalizes before returning — insensitive to a redundant `null` token in a
``levels`` list (S6.1).
"""

from __future__ import annotations

import hashlib
import json

from er.config.schema import Config

__all__ = ["canonical_document", "config_hash"]


def canonical_document(config: Config) -> str:
    """Return the canonical JSON serialisation S5.2 hashes.

    The caller passes the NORMALIZED document — the one :func:`er.config.loader.load_config`
    returns — because S6.1 normalization runs before the hash is computed.
    """
    return json.dumps(
        config.model_dump(mode="json", by_alias=False),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    )


def config_hash(config: Config) -> str:
    """SHA-256 hex digest of :func:`canonical_document`."""
    return hashlib.sha256(canonical_document(config).encode("utf-8")).hexdigest()
