"""The read-path connection pool: reuse, isolation, discard-on-error, eviction."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

import pytest
from erserver import readapi


class FakeConn:
    def __init__(self) -> None:
        self.closed = False


@pytest.fixture(autouse=True)
def fake_lake(monkeypatch: pytest.MonkeyPatch) -> list[FakeConn]:
    created: list[FakeConn] = []

    @contextmanager
    def fake_connect() -> Iterator[FakeConn]:
        conn = FakeConn()
        created.append(conn)
        try:
            yield conn
        finally:
            conn.closed = True

    @contextmanager
    def fake_env(env: dict[str, str]) -> Iterator[None]:
        yield

    monkeypatch.setattr(readapi, "connect", fake_connect)
    monkeypatch.setattr(readapi, "lake_environment", fake_env)
    readapi.close_pool()
    yield created
    readapi.close_pool()


ENV_A = {"ER_LAKE_METADATA_SCHEMA": "a"}
ENV_B = {"ER_LAKE_METADATA_SCHEMA": "b"}


def test_sequential_requests_reuse_one_attach(fake_lake: list[FakeConn]) -> None:
    with readapi.open_lake(ENV_A) as first:
        pass
    with readapi.open_lake(ENV_A) as second:
        pass
    assert first is second
    assert len(fake_lake) == 1
    assert not first.closed  # type: ignore[union-attr]


def test_tenants_never_share_an_attach(fake_lake: list[FakeConn]) -> None:
    with readapi.open_lake(ENV_A) as a:
        pass
    with readapi.open_lake(ENV_B) as b:
        pass
    assert a is not b
    assert len(fake_lake) == 2


def test_concurrent_requests_for_one_tenant_get_separate_attaches(
    fake_lake: list[FakeConn],
) -> None:
    with readapi.open_lake(ENV_A) as outer, readapi.open_lake(ENV_A) as inner:
        assert outer is not inner
    # both return to the pool and later requests reuse them
    with readapi.open_lake(ENV_A):
        pass
    assert len(fake_lake) == 2


def test_a_failing_request_discards_its_attach(fake_lake: list[FakeConn]) -> None:
    with pytest.raises(RuntimeError):
        with readapi.open_lake(ENV_A) as broken:
            raise RuntimeError("query blew up")
    assert broken.closed  # type: ignore[union-attr]
    with readapi.open_lake(ENV_A) as fresh:
        pass
    assert fresh is not broken
    assert len(fake_lake) == 2


def test_eviction_closes_the_least_recently_used(
    fake_lake: list[FakeConn], monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(readapi, "_POOL_CAPACITY", 2)
    envs: list[dict[str, Any]] = [{"T": "1"}, {"T": "2"}, {"T": "3"}]
    for env in envs:
        with readapi.open_lake(env):
            pass
    assert len(fake_lake) == 3
    assert fake_lake[0].closed  # the oldest was evicted
    assert not fake_lake[1].closed and not fake_lake[2].closed


def test_close_pool_closes_everything(fake_lake: list[FakeConn]) -> None:
    with readapi.open_lake(ENV_A):
        pass
    with readapi.open_lake(ENV_B):
        pass
    readapi.close_pool()
    assert all(conn.closed for conn in fake_lake)
