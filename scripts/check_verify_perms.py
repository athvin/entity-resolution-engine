#!/usr/bin/env python3
"""Assert every ticket verify command is runnable under the loop's permission sandbox.

The driver hands the agent a `--settings` allow list and then asks it, at step 6, to
run each ticket's own `verify` command verbatim. If that command matches no allow
rule, `dontAsk` refuses it before the process starts. The agent cannot fix it, cannot
substitute a cheaper command without violating a standing rule, and correctly blocks
`environment` -- burning an attempt. Three of those and the ticket auto-blocks for a
reason that has nothing to do with its content.

All of that is knowable before a single iteration runs: the board declares every
verify command and the driver builds the allow list. This closes the loop.

Reads the settings JSON on stdin. Exit 0 = every verify command is covered.
Exit 1 = at least one is not; the offenders are printed.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

BOARD = Path("docs/implementation")
# Shell operators that separate one command from the next. A verify command is a
# chain, and EVERY link needs its own allow rule.
SEPARATORS = re.compile(r"&&|\|\||(?<!\|)\|(?!\|)|;")


def allow_prefixes(settings: dict) -> list[tuple[str, bool]]:
    """(pattern-body, is_glob) for every Bash(...) allow rule."""
    out: list[tuple[str, bool]] = []
    for rule in settings.get("permissions", {}).get("allow", []):
        if not (rule.startswith("Bash(") and rule.endswith(")")):
            continue
        body = rule[5:-1]
        if body.endswith(" *"):
            out.append((body[:-2], True))
        else:
            out.append((body, False))
    return out


def covered(command: str, rules: list[tuple[str, bool]]) -> bool:
    command = command.strip()
    if not command:
        return True
    for body, is_glob in rules:
        if is_glob:
            # `cmd *` covers `cmd <args>`; it does not cover a bare `cmd`.
            if command.startswith(body + " "):
                return True
        elif command == body:
            return True
    return False


def verify_commands() -> list[tuple[str, str, str]]:
    """(ticket_id, status, verify) for every ticket that still has work to do."""
    rows: list[tuple[str, str, str]] = []
    for path in sorted(BOARD.glob("ER-*.md")):
        text = path.read_text(encoding="utf-8")
        ident = re.search(r"^id: (\S+)", text, re.M)
        status = re.search(r"^status: (\S+)", text, re.M)
        verify = re.search(r'^verify: "(.*)"$', text, re.M)
        if not (ident and status and verify):
            continue
        if status.group(1) == "done":
            continue
        rows.append((ident.group(1), status.group(1), verify.group(1)))
    return rows


def main() -> int:
    try:
        settings = json.loads(sys.stdin.read())
    except json.JSONDecodeError as exc:
        print(f"check_verify_perms: settings JSON is not parseable: {exc}", file=sys.stderr)
        return 1

    rules = allow_prefixes(settings)
    if not rules:
        print("check_verify_perms: the allow list contains no Bash rules", file=sys.stderr)
        return 1

    offenders: list[str] = []
    for ident, _status, verify in verify_commands():
        for part in SEPARATORS.split(verify):
            part = part.strip()
            if part and not covered(part, rules):
                offenders.append(f"  {ident}: {part}")

    if offenders:
        print(f"{len(offenders)} verify sub-command(s) match no allow rule:", file=sys.stderr)
        for line in offenders:
            print(line, file=sys.stderr)
        print(
            "\nEach of these would be denied at step 6, cost the ticket an attempt, and\n"
            "auto-block it after three. Add the matching Bash(...) rules to the allow\n"
            "list in scripts/run-loop.sh.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
