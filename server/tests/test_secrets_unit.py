"""Secret references resolve from the server environment, never the database."""

from __future__ import annotations

import pytest
from erserver.secrets import UnresolvedSecretError, resolve_env, resolve_value


def test_literals_pass_through_untouched() -> None:
    assert resolve_value("plain-value") == "plain-value"
    assert resolve_env({"A": "1", "B": "two"}) == {"A": "1", "B": "two"}


def test_references_resolve_from_server_env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ERSERVER_SECRET_S3_KEY", "the-actual-secret")
    assert resolve_value("secret://S3_KEY") == "the-actual-secret"
    resolved = resolve_env({"ER_S3_SECRET_ACCESS_KEY": "secret://S3_KEY", "ER_X": "lit"})
    assert resolved == {"ER_S3_SECRET_ACCESS_KEY": "the-actual-secret", "ER_X": "lit"}


def test_missing_reference_names_the_reference_not_a_value(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("ERSERVER_SECRET_NOPE", raising=False)
    with pytest.raises(UnresolvedSecretError) as caught:
        resolve_value("secret://NOPE")
    assert "secret://NOPE" in str(caught.value)
    assert "ERSERVER_SECRET_NOPE" in str(caught.value)


def test_empty_server_secret_counts_as_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ERSERVER_SECRET_BLANK", "   ")
    with pytest.raises(UnresolvedSecretError):
        resolve_value("secret://BLANK")
