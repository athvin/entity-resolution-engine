#!/usr/bin/env python3
"""Offline proof that this image's DuckDB extensions are baked, loaded and pinned.

S4.0b installs `ducklake`, `postgres` and `httpfs` at Docker **build** time into
``/opt/duckdb_extensions`` so that every runtime connection can hold
``autoinstall_known_extensions = false``. That only holds if the directory the image
ships really contains the three extensions S2.1 pins, which is what this script
asserts -- it is run with ``--network none``, so a connection that quietly downloaded
a replacement could not make it pass.

This is deliberately not routed through `er doctor` (ER-022): the doctor needs a
catalog and an object store, and this has to run on a bare image with no services
and no network at all.

Exit status is 0 when every extension is installed, loaded and at its pinned
``extension_version``, and 1 otherwise -- with one line per extension either way, so
a failing image says *which* extension drifted rather than only that one did.
"""

from __future__ import annotations

import os
import sys
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Final

import duckdb

from er.versions import EXTENSION_PINS, ExtensionPin

#: S4.0b's baked directory, and the environment variable that overrides it. The
#: override exists for exactly one reason: pointing the check at an empty directory
#: is how the build proves the check is not vacuous, and a check nothing can make
#: fail asserts nothing.
EXTENSION_DIRECTORY_ENV: Final[str] = "ER_DUCKDB_EXTENSION_DIRECTORY"
DEFAULT_EXTENSION_DIRECTORY: Final[str] = "/opt/duckdb_extensions"


@dataclass(frozen=True, slots=True)
class ExtensionStatus:
    """What ``duckdb_extensions()`` reports for one pinned extension.

    ``load_error`` is a field rather than an exception because a missing extension
    must not stop the two after it from being reported: the caller's contract is one
    line per extension, then exit 1.
    """

    pin: ExtensionPin
    installed: bool
    loaded: bool
    version: str | None
    load_error: str | None

    @property
    def ok(self) -> bool:
        return (
            self.load_error is None
            and self.installed
            and self.loaded
            and self.version == self.pin.version
        )

    def line(self) -> str:
        version = self.version if self.version is not None else "-"
        detail = f" error={self.load_error}" if self.load_error is not None else ""
        return (
            f"{'ok  ' if self.ok else 'FAIL'} {self.pin.install_name:<9}"
            f" as {self.pin.registered_name:<16}"
            f" version={version} expected={self.pin.version}"
            f" installed={self.installed} loaded={self.loaded}{detail}"
        )


def check(
    directory: str = DEFAULT_EXTENSION_DIRECTORY,
    pins: Mapping[str, ExtensionPin] = EXTENSION_PINS,
) -> tuple[ExtensionStatus, ...]:
    """LOAD every pinned extension out of ``directory`` and report what DuckDB saw.

    The connection is opened with the three S4.0b settings that make this an offline
    check: the baked directory, and both autoinstall and autoload off, so a LOAD that
    succeeds proves the binary was already on disk.

    ``pins`` is a parameter so the negative arm of the build can pass a perturbed
    hash and watch this fail.
    """
    connection = duckdb.connect(
        ":memory:",
        config={
            "extension_directory": directory,
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": False,
        },
    )
    try:
        load_errors: dict[str, str] = {}
        for pin in pins.values():
            try:
                # The name comes from the S2.1 pin table, never from user input.
                connection.execute(f"LOAD {pin.install_name}")
            except duckdb.Error as exc:
                load_errors[pin.install_name] = str(exc).splitlines()[0]

        # `postgres` installs under one name and registers under another, so this
        # map is keyed on what duckdb_extensions() reports and looked up by
        # `registered_name` below -- a lookup by install name would find no row and
        # pass while asserting nothing (S2.1).
        reported = {
            str(name): (bool(installed), bool(loaded), None if version is None else str(version))
            for name, installed, loaded, version in connection.execute(
                "SELECT extension_name, installed, loaded, extension_version "
                "FROM duckdb_extensions()"
            ).fetchall()
        }
    finally:
        connection.close()

    statuses: list[ExtensionStatus] = []
    for pin in pins.values():
        installed, loaded, version = reported.get(pin.registered_name, (False, False, None))
        statuses.append(
            ExtensionStatus(
                pin=pin,
                installed=installed,
                loaded=loaded,
                version=version,
                load_error=load_errors.get(pin.install_name),
            )
        )
    return tuple(statuses)


def main() -> int:
    directory = os.environ.get(EXTENSION_DIRECTORY_ENV) or DEFAULT_EXTENSION_DIRECTORY
    statuses = check(directory, EXTENSION_PINS)

    print(f"extension_directory={directory}")
    for status in statuses:
        print(status.line())

    failed = [status for status in statuses if not status.ok]
    if failed:
        names = ", ".join(status.pin.install_name for status in failed)
        print(
            f"FAIL: {len(failed)} of {len(statuses)} extension(s) not baked at the S2.1 pin:"
            f" {names}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
