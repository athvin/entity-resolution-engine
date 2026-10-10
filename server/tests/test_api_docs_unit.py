"""The FastAPI docs routes are served in dev only (docs/infrastructure.md §4.1).

Bare units: the clients are used without their context manager, so the app's
lifespan — and its control-plane Postgres connection — never runs.
"""

from __future__ import annotations

import pytest
from erserver.api import create_app
from erserver.settings import ServerSettings
from fastapi.testclient import TestClient

DOC_PATHS = ("/docs", "/redoc", "/openapi.json")


def client_for(environment: str) -> TestClient:
    return TestClient(
        create_app(ServerSettings(dsn="postgresql://unused/cp", environment=environment))
    )


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_routes_are_served_in_dev(path: str) -> None:
    assert client_for("dev").get(path).status_code == 200


@pytest.mark.parametrize("path", DOC_PATHS)
def test_docs_routes_are_404_outside_dev(path: str) -> None:
    assert client_for("prod").get(path).status_code == 404
