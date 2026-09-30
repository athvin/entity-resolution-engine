"""The one SMTP stack of the platform (docs/backend-design.md §6, workstream A).

Every email — notification, digest, invite, password reset, report link —
leaves through :func:`send`, configured by ``ERSERVER_SMTP_URL``. Deliberately
stdlib ``smtplib`` against a relay (SES, Postmark, any submission endpoint):
deliverability — SPF, DKIM, reputation — is the relay's job and ops
configuration, never engine code.

The URL's userinfo halves may be ``secret://`` references; they are resolved at
send time through the same :mod:`erserver.secrets` seam every other credential
uses, so the control-plane database and the settings object never hold a
plaintext SMTP password.
"""

from __future__ import annotations

import smtplib
import ssl
from dataclasses import dataclass
from email.message import EmailMessage
from urllib.parse import unquote, urlparse

from erserver.secrets import resolve_value
from erserver.settings import ServerSettings

__all__ = ["MailerConfigError", "SmtpEndpoint", "configured", "endpoint_from", "send"]


class MailerConfigError(RuntimeError):
    """``ERSERVER_SMTP_URL`` (or the From address) is malformed or missing."""


@dataclass(frozen=True)
class SmtpEndpoint:
    """A parsed relay URL: where to connect and how to authenticate."""

    host: str
    port: int
    username: str | None
    password: str | None
    implicit_tls: bool


def endpoint_from(url: str) -> SmtpEndpoint:
    """Parse ``smtp://`` / ``smtps://`` into an endpoint, refusing anything else."""
    parsed = urlparse(url)
    if parsed.scheme not in ("smtp", "smtps"):
        raise MailerConfigError(
            f"ERSERVER_SMTP_URL must be smtp:// or smtps://, got {parsed.scheme!r}"
        )
    if not parsed.hostname:
        raise MailerConfigError("ERSERVER_SMTP_URL carries no host")
    implicit = parsed.scheme == "smtps"
    return SmtpEndpoint(
        host=parsed.hostname,
        port=parsed.port or (465 if implicit else 587),
        username=unquote(parsed.username) if parsed.username else None,
        password=unquote(parsed.password) if parsed.password else None,
        implicit_tls=implicit,
    )


def configured(settings: ServerSettings) -> bool:
    """Whether email can leave at all; ``False`` parks outbox rows as queued."""
    return bool(settings.smtp_url and settings.email_from)


def send(settings: ServerSettings, *, to: str, subject: str, body: str) -> None:
    """Deliver one plain-text message through the configured relay.

    Raises:
        MailerConfigError: no relay or From address is configured — the caller
            (the outbox drain) checks :func:`configured` first, so reaching this
            is a caller bug rather than an operational state.
        OSError, smtplib.SMTPException: the relay refused or the network failed;
            the outbox drain records the message and retries with backoff.
    """
    if settings.smtp_url is None or settings.email_from is None:
        raise MailerConfigError("no SMTP relay configured")
    endpoint = endpoint_from(settings.smtp_url)
    message = EmailMessage()
    message["From"] = settings.email_from
    message["To"] = to
    message["Subject"] = subject
    message.set_content(body)

    context = ssl.create_default_context()
    if endpoint.implicit_tls:
        client: smtplib.SMTP = smtplib.SMTP_SSL(endpoint.host, endpoint.port, context=context)
    else:
        client = smtplib.SMTP(endpoint.host, endpoint.port)
    try:
        if not endpoint.implicit_tls:
            client.starttls(context=context)
        if endpoint.username is not None and endpoint.password is not None:
            client.login(resolve_value(endpoint.username), resolve_value(endpoint.password))
        client.send_message(message)
    finally:
        client.quit()
