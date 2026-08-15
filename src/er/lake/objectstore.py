"""The S3 client the lake needs outside DuckDB (DesignDoc.md S4.0, S4.0b, S7.1).

S2.1's ``boto3`` row states the reason this module exists: DuckDB's ``httpfs`` can
read and write objects but cannot *enumerate* or *delete* a prefix, and the S4.0b
connection model forbids this file from opening a DuckDB connection at all. Both
``er lake reset`` (S4.0) and the S8.1 harness teardown need exactly those two
operations, so they are here and they go through boto3.

Two things about the client are easy to get wrong and are handled once, here:

* ``ER_S3_ENDPOINT`` is ``host:port`` with **no** scheme (S7.1) — DuckDB's httpfs
  rejects a URL in that position, so the variable cannot carry one. boto3 requires
  the opposite: ``endpoint_url`` must be a URL. The scheme is therefore derived from
  ``ER_S3_USE_SSL`` *by this module*, never by a caller, so the same variable can
  feed both engines.
* MinIO is addressed path-style (``ER_S3_URL_STYLE: path``, S7.1). boto3's default
  addressing is ``auto``, which promotes a valid bucket name into the hostname and
  would resolve ``lake.objectstore`` instead of ``objectstore`` — a DNS failure with
  no obvious connection to its cause.

:class:`PrefixGuardError` is the one piece of defensive code in the module and it is
not optional: ``delete_prefix`` is called by ``er lake reset`` and by harness
teardown, and a bucket-wide delete is unrecoverable.
"""

from __future__ import annotations

from typing import Any, Final

import boto3
from botocore.config import Config

from er.lake.env import InvalidEnvError, require_bool_env, require_env

__all__ = ["ObjectStore", "PrefixGuardError"]

_S3_SCHEME: Final[str] = "s3://"

# The `DeleteObjects` API caps one request at 1000 keys, and `ListObjectsV2` caps
# one page at the same number. Both are protocol limits, not tuning knobs.
_DELETE_BATCH: Final[int] = 1000

# S4.0b's `URL_STYLE` vocabulary on the left, boto3's `addressing_style` on the
# right. The two engines spell the same choice differently, which is the whole
# reason the mapping is written out rather than passed through.
_ADDRESSING_STYLES: Final[dict[str, str]] = {"path": "path", "vhost": "virtual"}


class PrefixGuardError(Exception):
    """A destructive call named a prefix at or above a bucket root.

    Raised *before* anything is listed or deleted. It carries no ``code``, so
    :func:`er.errors.exit_code_for` maps it to exit ``1``: a refused delete is an
    operation that was attempted and failed, not a bad config.
    """


def _split_uri(uri: str) -> tuple[str, str]:
    """Split ``s3://bucket/key`` into its bucket and key."""
    if not uri.startswith(_S3_SCHEME):
        raise ValueError(f"{uri!r} is not an s3:// URI")
    bucket, _, key = uri[len(_S3_SCHEME) :].partition("/")
    if not bucket:
        raise ValueError(f"{uri!r} names no bucket")
    return bucket, key


def _split_object_uri(uri: str) -> tuple[str, str]:
    """Split an s3:// URI that must address an object rather than a bucket."""
    bucket, key = _split_uri(uri)
    if not key:
        raise ValueError(f"{uri!r} names a bucket, not an object")
    return bucket, key


class ObjectStore:
    """S3 operations on ``DATA_PATH`` and on the model-artifact prefix.

    Constructed from an already-configured boto3 client so tests and `er doctor`
    can share one; :meth:`from_env` is how the pipeline builds it.
    """

    def __init__(self, client: Any) -> None:
        self._client = client

    @classmethod
    def from_env(cls) -> ObjectStore:
        """Build a client from the six ``ER_S3_*`` variables of S6.1.

        Reads every variable through :mod:`er.lake.env`, so an absent one fails as
        ``ERR_ENV_MISSING: <name>`` with exit ``2`` rather than as an opaque boto3
        connection error later.
        """
        endpoint = require_env("ER_S3_ENDPOINT")
        access_key_id = require_env("ER_S3_ACCESS_KEY_ID")
        secret_access_key = require_env("ER_S3_SECRET_ACCESS_KEY")
        region = require_env("ER_S3_REGION")
        url_style = require_env("ER_S3_URL_STYLE")
        use_ssl = require_bool_env("ER_S3_USE_SSL")

        if "://" in endpoint:
            raise InvalidEnvError("ER_S3_ENDPOINT", endpoint, "host:port with no scheme")
        addressing_style = _ADDRESSING_STYLES.get(url_style.strip().lower())
        if addressing_style is None:
            raise InvalidEnvError("ER_S3_URL_STYLE", url_style, "'path' or 'vhost'")

        scheme = "https" if use_ssl else "http"
        client = boto3.client(
            "s3",
            endpoint_url=f"{scheme}://{endpoint}",
            aws_access_key_id=access_key_id,
            aws_secret_access_key=secret_access_key,
            region_name=region,
            use_ssl=use_ssl,
            config=Config(
                signature_version="s3v4",
                s3={"addressing_style": addressing_style},
            ),
        )
        return cls(client)

    def put_bytes(self, uri: str, data: bytes) -> None:
        """Write ``data`` to ``uri``, replacing whatever was there."""
        bucket, key = _split_object_uri(uri)
        self._client.put_object(Bucket=bucket, Key=key, Body=data)

    def get_bytes(self, uri: str) -> bytes:
        """Read ``uri`` in full."""
        bucket, key = _split_object_uri(uri)
        response = self._client.get_object(Bucket=bucket, Key=key)
        # Annotated rather than returned directly: boto3 ships no type information,
        # so the response is `Any` and `--strict`'s warn_return_any would otherwise
        # let a non-bytes body through the signature unnoticed.
        body: bytes = response["Body"].read()
        return body

    def list_prefix(self, prefix: str) -> list[str]:
        """Every object under ``prefix``, as ``s3://`` URIs, in listing order.

        Paginated: S3 returns at most 1000 keys per page, and a namespace prefix
        routinely holds more, so a single ``list_objects_v2`` call would silently
        report a truncated namespace as a clean one.
        """
        bucket, key = _split_uri(prefix)
        uris: list[str] = []
        for page in self._client.get_paginator("list_objects_v2").paginate(
            Bucket=bucket, Prefix=key
        ):
            for entry in page.get("Contents", ()):
                uris.append(f"{_S3_SCHEME}{bucket}/{entry['Key']}")
        return uris

    def delete_prefix(self, prefix: str) -> int:
        """Delete every object under ``prefix`` and return how many were removed.

        Refuses a prefix at or above a bucket root with :class:`PrefixGuardError`,
        before listing anything. ``er lake reset`` and the S8.1 harness teardown are
        the two callers, both of them destructive and both of them driven by an
        environment variable that a typo can empty.
        """
        bucket, key = self._guard(prefix)
        keys = [_split_object_uri(uri)[1] for uri in self.list_prefix(prefix)]
        for start in range(0, len(keys), _DELETE_BATCH):
            batch = keys[start : start + _DELETE_BATCH]
            self._client.delete_objects(
                Bucket=bucket,
                Delete={"Objects": [{"Key": k} for k in batch], "Quiet": True},
            )
        return len(keys)

    @staticmethod
    def _guard(prefix: str) -> tuple[str, str]:
        """Split ``prefix`` after refusing anything shallower than a namespace."""
        try:
            bucket, key = _split_uri(prefix)
        except ValueError as exc:
            raise PrefixGuardError(f"refusing to delete {prefix!r}: {exc}") from exc
        # `s3://lake` and `s3://lake/` both reduce to an empty key, and so does
        # `s3://lake//`. All three would delete the bucket.
        if not key.strip("/"):
            raise PrefixGuardError(
                f"refusing to delete {prefix!r}: a bucket root is never a namespace prefix"
            )
        return bucket, key
