#!/usr/bin/env python3
"""Deterministic CLI for the Entity Resolution implementation board.

This script owns EVERY state transition and EVERY frontmatter field of every
ticket in ``docs/implementation``. The implementation loop never edits ticket
frontmatter by hand: it calls this script, which validates the transition,
writes the file, and commits the change itself.

Stdlib only, and deliberately so -- ER-003 is the ticket that creates the venv,
so this must run on a checkout that has no virtualenv yet.

Exit codes (shared with the ``er`` CLI contract in DesignDoc S4.0):
    0   success
    1   refused: the transition is not legal, or validation found defects
    2   usage / malformed input
    3   precondition failure (dirty tree, wrong branch, lock held)
    10  nothing to do (no READY ticket)
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

EXIT_OK = 0
EXIT_REFUSED = 1
EXIT_USAGE = 2
EXIT_PRECONDITION = 3
EXIT_NOTHING_TO_DO = 10

REPO_ROOT = Path(__file__).resolve().parent.parent
BOARD_DIR = REPO_ROOT / "docs" / "implementation"
LOOP_DIR = REPO_ROOT / ".loop"
RECEIPTS_DIR = LOOP_DIR / "receipts"
LOCK_PATH = LOOP_DIR / "board.lock"
SPEC_PATH = REPO_ROOT / "DesignDoc.md"
INTERFACES_PATH = BOARD_DIR / "INTERFACES.md"
BLOCKED_LOG = LOOP_DIR / "blocked.log"

TICKET_RE = re.compile(r"^ER-(\d{3})-([a-z0-9][a-z0-9-]*)\.md$")
ID_RE = re.compile(r"^ER-\d{3}$")

STATUSES = ("todo", "in_progress", "blocked", "done")
SIZES = ("S", "M", "L")
KINDS = ("code", "spec-amendment", "docs", "fixture")
GATES = ("fast", "full")

# Blocking classes. ``environment`` is special: it is not the ticket's fault, so
# the ticket returns to ``todo`` and the attempt is refunded. Without this, one
# missing Docker daemon permanently blocks every integration ticket on the board.
BLOCK_CLASSES = (
    "verify_failed",
    "gate_failed",
    "underspecified",
    "spec_contradiction",
    "max_attempts",
    "environment",
)
REFUNDING_CLASSES = ("environment",)

MAX_ATTEMPTS = 3

# Fields written to every ticket, in this order. Order is load-bearing: it keeps
# diffs readable and makes the round-trip byte-stable.
FIELD_ORDER = [
    "id",
    "title",
    "milestone",
    "status",
    "kind",
    "size",
    "gates",
    "depends_on",
    "spec_refs",
    "gap_refs",
    "provides",
    "consumes",
    "owns",
    "protected_paths",
    "extra_paths",
    "allowed_suppressions",
    "attempts",
    "verify",
    "branch",
    "commit",
    "spec_sha",
    "updated_at",
]

LIST_FIELDS = {
    "depends_on",
    "spec_refs",
    "gap_refs",
    "provides",
    "consumes",
    "owns",
    "protected_paths",
    "extra_paths",
    # Declared per ticket when a suppression is genuinely unavoidable. It must be a
    # real list field: when it was undeclared, `ticket_field` returned the empty
    # string, gates.sh compared that against "[]", and the suppression gate never
    # fired for any ticket on the board.
    "allowed_suppressions",
}
INT_FIELDS = {"attempts"}
REQUIRED_FIELDS = {"id", "title", "milestone", "status", "size", "verify"}

# Scalars that MUST be double-quoted on disk. ``verify`` routinely contains ':'
# and '#', and an unquoted '#' in a YAML flow scalar is a comment -- a real
# parser would silently truncate the command.
MUST_QUOTE = {"title", "verify", "branch", "commit", "spec_sha", "updated_at"}


class BoardError(Exception):
    """A refused transition or a malformed board. Carries an exit code."""

    def __init__(self, message: str, code: int = EXIT_REFUSED) -> None:
        super().__init__(message)
        self.code = code


# --------------------------------------------------------------------------
# git helpers
# --------------------------------------------------------------------------


def git(*args: str, check: bool = True, cwd: Path | None = None) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(cwd or REPO_ROOT),
        capture_output=True,
        text=True,
    )
    if check and proc.returncode != 0:
        raise BoardError(
            f"git {' '.join(args)} failed ({proc.returncode}): {proc.stderr.strip()}"
        )
    return proc.stdout.strip()


def git_ok(*args: str) -> bool:
    proc = subprocess.run(
        ["git", *args], cwd=str(REPO_ROOT), capture_output=True, text=True
    )
    return proc.returncode == 0


def working_tree_dirty(exclude: Iterable[Path] = ()) -> list[str]:
    """Return porcelain lines for a dirty tree, ignoring the given paths."""
    excluded = {str(p.relative_to(REPO_ROOT)) for p in exclude}
    lines = []
    for line in git("status", "--porcelain").splitlines():
        path = line[3:].strip()
        if path in excluded:
            continue
        lines.append(line)
    return lines


def current_branch() -> str:
    return git("rev-parse", "--abbrev-ref", "HEAD")


def head_tree() -> str:
    return git("rev-parse", "HEAD^{tree}")


def commit_subject(sha: str) -> str:
    return git("show", "-s", "--format=%s", sha)


def commit_exists(sha: str) -> bool:
    return git_ok("cat-file", "-e", f"{sha}^{{commit}}")


def commit_touches_files(sha: str) -> int:
    out = git("show", "--name-only", "--format=", sha)
    return len([line for line in out.splitlines() if line.strip()])


def spec_sha() -> str:
    if not SPEC_PATH.exists():
        return ""
    return hashlib.sha256(SPEC_PATH.read_bytes()).hexdigest()[:16]


def commit_board_change(paths: list[Path], message: str) -> None:
    """Commit the board's own writes.

    This is not optional. If ``claim`` left the ticket file modified but
    uncommitted, the next iteration's preflight would see a dirty tree and
    refuse -- the loop would deadlock after exactly one ticket.
    """
    rel = [str(p.relative_to(REPO_ROOT)) for p in paths if p.exists()]
    if not rel:
        return
    git("add", "--", *rel)
    staged = git("diff", "--cached", "--name-only")
    if not staged.strip():
        return
    git("commit", "--only", "-m", message, "--", *rel)


# --------------------------------------------------------------------------
# locking
# --------------------------------------------------------------------------


class BoardLock:
    """Advisory lock so two loops cannot interleave board writes.

    ``flock(1)`` is absent on macOS, so this uses fcntl directly rather than
    shelling out.
    """

    def __init__(self, path: Path = LOCK_PATH) -> None:
        self.path = path
        self.handle = None

    def __enter__(self) -> "BoardLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.handle = open(self.path, "w")
        try:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.handle.close()
            raise BoardError(
                f"another board operation holds {self.path}; refusing to interleave",
                EXIT_PRECONDITION,
            )
        self.handle.write(f"{os.getpid()}\n")
        self.handle.flush()
        return self

    def __exit__(self, *exc: object) -> None:
        if self.handle is not None:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
            self.handle.close()


# --------------------------------------------------------------------------
# frontmatter: a deliberately restricted YAML subset
# --------------------------------------------------------------------------


def _parse_scalar(raw: str, field_name: str, where: str) -> str:
    raw = raw.strip()
    if raw.startswith('"') and raw.endswith('"') and len(raw) >= 2:
        return raw[1:-1].replace('\\"', '"').replace("\\\\", "\\")
    if raw.startswith("'") and raw.endswith("'") and len(raw) >= 2:
        return raw[1:-1]
    if field_name in MUST_QUOTE and raw != "":
        raise BoardError(
            f"{where}: field '{field_name}' must be double-quoted (got: {raw})"
        )
    if "#" in raw:
        raise BoardError(
            f"{where}: field '{field_name}' has an unquoted '#'; quote the value"
        )
    return raw


def _parse_list(raw: str, field_name: str, where: str) -> list[str]:
    raw = raw.strip()
    if raw in ("", "[]"):
        return []
    if not (raw.startswith("[") and raw.endswith("]")):
        raise BoardError(f"{where}: field '{field_name}' must be a flow list [a, b]")
    inner = raw[1:-1].strip()
    if not inner:
        return []
    items: list[str] = []
    for part in _split_flow(inner):
        part = part.strip()
        if not part:
            continue
        if not (part.startswith('"') and part.endswith('"')):
            raise BoardError(
                f"{where}: every element of '{field_name}' must be double-quoted "
                f"(got: {part}). An unquoted '#' or ':' is not safe in YAML."
            )
        items.append(part[1:-1].replace('\\"', '"'))
    return items


def _split_flow(text: str) -> list[str]:
    """Split a flow sequence on commas that are not inside double quotes."""
    parts: list[str] = []
    buf: list[str] = []
    in_quotes = False
    escaped = False
    for ch in text:
        if escaped:
            buf.append(ch)
            escaped = False
            continue
        if ch == "\\":
            buf.append(ch)
            escaped = True
            continue
        if ch == '"':
            in_quotes = not in_quotes
            buf.append(ch)
            continue
        if ch == "," and not in_quotes:
            parts.append("".join(buf))
            buf = []
            continue
        buf.append(ch)
    parts.append("".join(buf))
    return parts


def _emit_scalar(field_name: str, value: Any) -> str:
    if value is None:
        value = ""
    if field_name in INT_FIELDS:
        return str(int(value))
    text = str(value)
    if field_name in MUST_QUOTE or text == "" or any(c in text for c in ':#"\'{}[]'):
        escaped = text.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'
    return text


def _emit_list(values: list[str]) -> str:
    if not values:
        return "[]"
    quoted = ['"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"' for v in values]
    return "[" + ", ".join(quoted) + "]"


@dataclass
class Ticket:
    path: Path
    fields: dict[str, Any]
    body: str
    defects: list[str] = field(default_factory=list)

    @property
    def id(self) -> str:
        return str(self.fields.get("id", ""))

    @property
    def number(self) -> int:
        try:
            return int(self.id.split("-")[1])
        except (IndexError, ValueError):
            return 10**6

    @property
    def status(self) -> str:
        return str(self.fields.get("status", ""))

    @property
    def attempts(self) -> int:
        return int(self.fields.get("attempts", 0) or 0)

    @property
    def depends_on(self) -> list[str]:
        return list(self.fields.get("depends_on", []))

    def slug(self) -> str:
        m = TICKET_RE.match(self.path.name)
        return m.group(2) if m else "ticket"

    def branch_name(self) -> str:
        return f"ticket/{self.id}-{self.slug()}"

    def render(self) -> str:
        lines = ["---"]
        for key in FIELD_ORDER:
            if key not in self.fields:
                continue
            value = self.fields[key]
            if key in LIST_FIELDS:
                lines.append(f"{key}: {_emit_list(list(value))}")
            else:
                lines.append(f"{key}: {_emit_scalar(key, value)}")
        for key in sorted(k for k in self.fields if k not in FIELD_ORDER):
            value = self.fields[key]
            if key in LIST_FIELDS:
                lines.append(f"{key}: {_emit_list(list(value))}")
            else:
                lines.append(f"{key}: {_emit_scalar(key, value)}")
        lines.append("---")
        text = "\n".join(lines) + "\n" + self.body
        if not text.endswith("\n"):
            text += "\n"
        return text

    def save(self) -> None:
        self.fields["updated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        tmp = self.path.with_suffix(".md.tmp")
        tmp.write_text(self.render(), encoding="utf-8")
        tmp.replace(self.path)


def load_ticket(path: Path, strict: bool = True) -> Ticket:
    raw = path.read_text(encoding="utf-8")
    if not raw.startswith("---\n"):
        raise BoardError(f"{path.name}: missing YAML frontmatter", EXIT_REFUSED)
    end = raw.find("\n---", 4)
    if end == -1:
        raise BoardError(f"{path.name}: unterminated frontmatter", EXIT_REFUSED)
    head = raw[4:end]
    body = raw[end + 4 :]
    if body.startswith("\n"):
        body = body[1:]

    fields: dict[str, Any] = {}
    defects: list[str] = []
    for lineno, line in enumerate(head.splitlines(), start=2):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if ":" not in line:
            defects.append(f"{path.name}:{lineno}: not a key: value pair")
            continue
        key, _, rest = line.partition(":")
        key = key.strip()
        where = f"{path.name}:{lineno}"
        try:
            if key in LIST_FIELDS:
                fields[key] = _parse_list(rest, key, where)
            elif key in INT_FIELDS:
                fields[key] = int(_parse_scalar(rest, key, where) or 0)
            else:
                fields[key] = _parse_scalar(rest, key, where)
        except BoardError as exc:
            defects.append(str(exc))
        except ValueError:
            defects.append(f"{where}: '{key}' must be an integer")

    ticket = Ticket(path=path, fields=fields, body=body, defects=defects)
    if strict and defects:
        raise BoardError("\n".join(defects), EXIT_REFUSED)
    return ticket


def load_board(board_dir: Path = BOARD_DIR, strict: bool = False) -> list[Ticket]:
    if not board_dir.is_dir():
        raise BoardError(
            f"board directory not found: {board_dir}\n"
            "git does not track empty directories; commit docs/implementation/README.md first.",
            EXIT_PRECONDITION,
        )
    tickets = []
    for path in sorted(board_dir.iterdir()):
        if not TICKET_RE.match(path.name):
            continue
        tickets.append(load_ticket(path, strict=strict))
    return tickets


def find_ticket(ticket_id: str, board_dir: Path = BOARD_DIR) -> Ticket:
    if not ID_RE.match(ticket_id):
        raise BoardError(f"malformed ticket id: {ticket_id}", EXIT_USAGE)
    for ticket in load_board(board_dir):
        if ticket.id == ticket_id:
            return ticket
    raise BoardError(f"no such ticket: {ticket_id}", EXIT_USAGE)


# --------------------------------------------------------------------------
# validate
# --------------------------------------------------------------------------


def spec_anchors() -> set[str]:
    if not SPEC_PATH.exists():
        return set()
    return set(re.findall(r'<a id="([^"]+)"></a>', SPEC_PATH.read_text(encoding="utf-8")))


def cmd_validate(args: argparse.Namespace) -> int:
    board_dir = Path(args.board_dir).resolve() if args.board_dir else BOARD_DIR
    tickets = load_board(board_dir)
    defects: list[str] = []
    for ticket in tickets:
        defects.extend(ticket.defects)

    if not tickets:
        print(f"no tickets found in {board_dir}", file=sys.stderr)
        return EXIT_REFUSED

    by_id: dict[str, Ticket] = {}
    for ticket in tickets:
        name = ticket.path.name
        for required in sorted(REQUIRED_FIELDS):
            if not ticket.fields.get(required):
                defects.append(f"{name}: missing required field '{required}'")
        if ticket.id and not ID_RE.match(ticket.id):
            defects.append(f"{name}: malformed id '{ticket.id}'")
        if ticket.id and not name.startswith(ticket.id + "-"):
            defects.append(f"{name}: filename does not match id '{ticket.id}'")
        if ticket.id in by_id:
            defects.append(f"{name}: duplicate id '{ticket.id}'")
        else:
            by_id[ticket.id] = ticket
        if ticket.status not in STATUSES:
            defects.append(f"{name}: status '{ticket.status}' not in {STATUSES}")
        if ticket.fields.get("size") not in SIZES:
            defects.append(f"{name}: size '{ticket.fields.get('size')}' not in {SIZES}")
        kind = ticket.fields.get("kind", "code")
        if kind and kind not in KINDS:
            defects.append(f"{name}: kind '{kind}' not in {KINDS}")
        gates = ticket.fields.get("gates", "fast")
        if gates and gates not in GATES:
            defects.append(f"{name}: gates '{gates}' not in {GATES}")
        if not str(ticket.fields.get("verify", "")).strip():
            defects.append(f"{name}: verify command is empty")
        if not ticket.fields.get("spec_refs"):
            defects.append(f"{name}: spec_refs is empty")
        if ticket.attempts < 0:
            defects.append(f"{name}: attempts is negative")
        if not re.search(r"^- \[[ x]\] AC1:", ticket.body, re.M):
            defects.append(
                f"{name}: acceptance criteria must be numbered '- [ ] AC1: ...' "
                "so plan-check can assert coverage"
            )
        if "## Definition of Done" not in ticket.body:
            defects.append(f"{name}: missing '## Definition of Done' section")

    # dangling dependencies
    for ticket in tickets:
        for dep in ticket.depends_on:
            if dep not in by_id:
                defects.append(f"{ticket.path.name}: depends_on unknown ticket '{dep}'")

    # cycles
    colour: dict[str, int] = {}
    stack: list[str] = []

    def visit(node: str) -> None:
        colour[node] = 1
        stack.append(node)
        for dep in by_id[node].depends_on if node in by_id else []:
            if dep not in by_id:
                continue
            if colour.get(dep, 0) == 1:
                cycle = stack[stack.index(dep) :] + [dep]
                defects.append("dependency cycle: " + " -> ".join(cycle))
            elif colour.get(dep, 0) == 0:
                visit(dep)
        stack.pop()
        colour[node] = 2

    for tid in by_id:
        if colour.get(tid, 0) == 0:
            visit(tid)

    # spec anchors
    anchors = spec_anchors()
    if anchors:
        for ticket in tickets:
            for ref in ticket.fields.get("spec_refs", []):
                if ref.startswith("gap:"):
                    continue
                anchor = ref.split("#")[-1] if "#" in ref else ref
                if anchor not in anchors:
                    defects.append(
                        f"{ticket.path.name}: spec_refs '{ref}' resolves to no anchor in DesignDoc.md"
                    )

    # git-reachability of done tickets
    if args.strict:
        for ticket in tickets:
            if ticket.status == "done" and ticket.fields.get("commit"):
                sha = str(ticket.fields["commit"])
                if not commit_exists(sha):
                    defects.append(f"{ticket.path.name}: commit {sha} does not exist")
                elif not commit_subject(sha).startswith(ticket.id):
                    defects.append(
                        f"{ticket.path.name}: commit {sha} subject does not start with {ticket.id}"
                    )
            if ticket.status == "in_progress" and ticket.fields.get("branch"):
                if not git_ok("rev-parse", "--verify", str(ticket.fields["branch"])):
                    defects.append(
                        f"{ticket.path.name}: in_progress but branch "
                        f"{ticket.fields['branch']} does not exist"
                    )

    if defects:
        for defect in defects:
            print(f"DEFECT {defect}", file=sys.stderr)
        print(f"\n{len(defects)} defect(s) in {len(tickets)} ticket(s)", file=sys.stderr)
        return EXIT_REFUSED

    print(f"OK {len(tickets)} tickets, no defects")
    return EXIT_OK


# --------------------------------------------------------------------------
# selection
# --------------------------------------------------------------------------


def unblocks_count(ticket: Ticket, tickets: list[Ticket]) -> int:
    """How many not-done tickets transitively depend on this one."""
    reverse: dict[str, list[str]] = {}
    for other in tickets:
        for dep in other.depends_on:
            reverse.setdefault(dep, []).append(other.id)
    seen: set[str] = set()
    frontier = list(reverse.get(ticket.id, []))
    while frontier:
        node = frontier.pop()
        if node in seen:
            continue
        seen.add(node)
        frontier.extend(reverse.get(node, []))
    by_id = {t.id: t for t in tickets}
    return sum(1 for n in seen if by_id.get(n) and by_id[n].status != "done")


def ready_tickets(tickets: list[Ticket]) -> list[Ticket]:
    done = {t.id for t in tickets if t.status == "done"}
    ready = []
    for ticket in tickets:
        if ticket.status != "todo":
            continue
        if ticket.attempts >= MAX_ATTEMPTS:
            continue
        if all(dep in done for dep in ticket.depends_on):
            ready.append(ticket)
    return ready


def selection_key(ticket: Ticket, tickets: list[Ticket]) -> tuple:
    # milestone ASC, unblocks DESC, attempts ASC, size ASC, id ASC.
    # unblocks must outrank attempts: ordering by attempts first starves exactly
    # the high-fan-out tickets whose completion makes the rest of the board READY.
    size_rank = {"S": 0, "M": 1, "L": 2}
    return (
        str(ticket.fields.get("milestone", "")),
        -unblocks_count(ticket, tickets),
        ticket.attempts,
        size_rank.get(str(ticket.fields.get("size")), 3),
        ticket.number,
    )


def cmd_next(args: argparse.Namespace) -> int:
    tickets = load_board()
    in_progress = [t for t in tickets if t.status == "in_progress"]
    if in_progress:
        payload = {
            "status": "board_busy",
            "in_progress": [t.id for t in in_progress],
        }
        _emit(payload, args.json, f"BUSY {', '.join(t.id for t in in_progress)}")
        return EXIT_REFUSED

    ready = ready_tickets(tickets)
    if not ready:
        blocked = [t.id for t in tickets if t.status == "blocked"]
        starved = [
            t.id
            for t in tickets
            if t.status == "todo" and t.attempts >= MAX_ATTEMPTS
        ]
        payload = {
            "status": "no_ready_tickets",
            "blocked": blocked,
            "starved": starved,
            "done": sum(1 for t in tickets if t.status == "done"),
            "total": len(tickets),
        }
        summary = (
            f"NO_READY_TICKETS done={payload['done']}/{payload['total']} "
            f"blocked={len(blocked)} starved={len(starved)}"
        )
        if blocked or starved:
            summary = "STALLED " + summary
        _emit(payload, args.json, summary)
        return EXIT_NOTHING_TO_DO

    chosen = sorted(ready, key=lambda t: selection_key(t, tickets))[0]
    payload = {
        "status": "ready",
        "id": chosen.id,
        "title": chosen.fields.get("title"),
        "milestone": chosen.fields.get("milestone"),
        "size": chosen.fields.get("size"),
        "gates": chosen.fields.get("gates", "fast"),
        "kind": chosen.fields.get("kind", "code"),
        "attempts": chosen.attempts,
        "unblocks": unblocks_count(chosen, tickets),
        "path": str(chosen.path.relative_to(REPO_ROOT)),
        "branch": chosen.branch_name(),
        "verify": chosen.fields.get("verify"),
    }
    _emit(payload, args.json, f"{chosen.id} {chosen.fields.get('title')}")
    return EXIT_OK


def _emit(payload: dict, as_json: bool, human: str) -> None:
    if as_json:
        print(json.dumps(payload, indent=2))
    else:
        print(human)


# --------------------------------------------------------------------------
# transitions
# --------------------------------------------------------------------------


def cmd_claim(args: argparse.Namespace) -> int:
    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status == "done":
            raise BoardError(f"{ticket.id} is already done", EXIT_REFUSED)
        if ticket.status == "blocked":
            raise BoardError(
                f"{ticket.id} is blocked; a human must run 'board.py unblock {ticket.id}'",
                EXIT_REFUSED,
            )
        if ticket.status == "in_progress":
            raise BoardError(f"{ticket.id} is already in_progress", EXIT_REFUSED)

        others = [t for t in load_board() if t.status == "in_progress"]
        if others:
            raise BoardError(
                f"board busy: {', '.join(t.id for t in others)} in_progress", EXIT_REFUSED
            )

        done = {t.id for t in load_board() if t.status == "done"}
        missing = [d for d in ticket.depends_on if d not in done]
        if missing:
            raise BoardError(
                f"{ticket.id} depends on unfinished tickets: {', '.join(missing)}",
                EXIT_REFUSED,
            )

        # attempts increments HERE, before any work happens, so a crash mid-ticket
        # still costs an attempt and cannot loop forever.
        attempts = ticket.attempts + 1
        if attempts > MAX_ATTEMPTS:
            ticket.fields["status"] = "blocked"
            ticket.fields["attempts"] = ticket.attempts
            _append_blocker(
                ticket,
                block_class="max_attempts",
                failing_command="(none)",
                assertion=f"attempts would exceed MAX_ATTEMPTS={MAX_ATTEMPTS}",
                smallest_fix="A human must review the blocker log and run 'board.py unblock'.",
                log=None,
            )
            ticket.save()
            commit_board_change([ticket.path], f"board({ticket.id}): blocked max_attempts")
            raise BoardError(
                f"{ticket.id} has reached MAX_ATTEMPTS={MAX_ATTEMPTS}; auto-blocked",
                EXIT_REFUSED,
            )

        ticket.fields["status"] = "in_progress"
        ticket.fields["attempts"] = attempts
        ticket.fields["branch"] = ticket.branch_name()
        ticket.fields["spec_sha"] = spec_sha()
        if args.session:
            ticket.fields["session"] = args.session
        ticket.save()
        commit_board_change(
            [ticket.path], f"board({ticket.id}): claim attempt {attempts}"
        )

    payload = {
        "status": "claimed",
        "id": ticket.id,
        "attempts": ticket.fields["attempts"],
        "branch": ticket.fields["branch"],
        "path": str(ticket.path.relative_to(REPO_ROOT)),
        "verify": ticket.fields.get("verify"),
        "gates": ticket.fields.get("gates", "fast"),
    }
    _emit(payload, args.json, f"CLAIMED {ticket.id} attempt {ticket.fields['attempts']}")
    return EXIT_OK


def _read_receipt(ticket: Ticket) -> dict | None:
    path = RECEIPTS_DIR / f"{ticket.id}-{ticket.attempts}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return None


def cmd_complete(args: argparse.Namespace) -> int:
    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status != "in_progress":
            raise BoardError(
                f"{ticket.id} is '{ticket.status}', not in_progress", EXIT_REFUSED
            )

        sha = args.commit
        if not commit_exists(sha):
            raise BoardError(f"commit {sha} does not exist", EXIT_REFUSED)
        subject = commit_subject(sha)
        if not subject.startswith(ticket.id):
            raise BoardError(
                f"commit {sha} subject '{subject}' does not start with {ticket.id}",
                EXIT_REFUSED,
            )
        if commit_touches_files(sha) < 1:
            raise BoardError(f"commit {sha} touches no files", EXIT_REFUSED)
        # The ticket file is NOT excluded. It carries the verify command, the
        # protected paths and the acceptance criteria that every enforcement point
        # reads live, so an uncommitted edit to it is precisely the change that
        # must block completion. board.py commits its own transitions, so a
        # legitimate run leaves it clean here.
        dirty = working_tree_dirty()
        if dirty:
            raise BoardError(
                "working tree is dirty; commit or discard before completing:\n"
                + "\n".join(dirty),
                EXIT_PRECONDITION,
            )

        # Receipt binding. gates.sh -- never the model -- writes the receipt, and
        # it records the tree it actually tested. Requiring that tree to equal the
        # committed tree is what makes "done" mean something: run gates, then edit
        # code, then commit, and the trees no longer match.
        receipt = _read_receipt(ticket)
        if receipt is None:
            raise BoardError(
                f"no gate receipt at .loop/receipts/{ticket.id}-{ticket.attempts}.json; "
                "run scripts/gates.sh before completing",
                EXIT_REFUSED,
            )
        committed_tree = git("rev-parse", f"{sha}^{{tree}}")
        if receipt.get("tree") != committed_tree:
            raise BoardError(
                "gate receipt is stale: it was produced against tree "
                f"{receipt.get('tree')} but the commit's tree is {committed_tree}. "
                "Re-run scripts/gates.sh against the committed tree.",
                EXIT_REFUSED,
            )
        # -2 means "the tooling this gate needs does not exist yet at this point in
        # the build". It is written only by gates.sh. It is acceptable EXCEPT for
        # the ticket whose own job is to create that tooling -- otherwise the
        # ticket that builds the integration runner could skip proving it works.
        codes = receipt.get("exit_codes") or {}
        owns = set(ticket.fields.get("owns", []))
        BOOTSTRAP_OWNERS = {
            "integration": "scripts/ci/itest.sh",
            "dbt": "dbt/dbt_project.yml",
            "lint": "pyproject.toml",
            "types": "pyproject.toml",
            "unit": "pyproject.toml",
        }
        bad: dict[str, int] = {}
        for gate, code in codes.items():
            if code == 0:
                continue
            if code == -2 and gate in BOOTSTRAP_OWNERS and BOOTSTRAP_OWNERS[gate] not in owns:
                print(
                    f"NOTE {ticket.id}: the '{gate}' gate was unavailable "
                    f"({BOOTSTRAP_OWNERS[gate]} does not exist yet) and was recorded as skipped.",
                    file=sys.stderr,
                )
                continue
            bad[gate] = code
        if bad:
            raise BoardError(f"gate receipt records failures: {bad}", EXIT_REFUSED)

        # The receipt must cover the ladder this ticket actually required, or a
        # narrow run could satisfy a broad ticket.
        required = {"hygiene", "spec", "board", "lint", "types", "unit", "dbt"}
        if str(ticket.fields.get("gates", "fast")) == "full":
            required.add("integration")
        missing_gates = sorted(required - set(codes))
        if missing_gates:
            raise BoardError(
                f"gate receipt does not cover: {', '.join(missing_gates)}; "
                "re-run scripts/gates.sh for this ticket",
                EXIT_REFUSED,
            )

        # The receipt must record the ticket's CURRENT verify command. If the
        # command changed after gates ran, the receipt proves nothing about it.
        if receipt.get("verify_cmd") != str(ticket.fields.get("verify", "")):
            raise BoardError(
                "gate receipt was produced against a different verify command than the "
                "ticket now declares:\n"
                f"  receipt: {receipt.get('verify_cmd')!r}\n"
                f"  ticket : {ticket.fields.get('verify')!r}",
                EXIT_REFUSED,
            )
        if receipt.get("verify_exit") != 0:
            raise BoardError(
                f"gate receipt records verify_exit={receipt.get('verify_exit')}",
                EXIT_REFUSED,
            )

        # Re-run the ticket's own verify here, unconditionally for fast-scope
        # tickets. This removes the receipt-forgery path entirely for most of the
        # board. There is deliberately no flag to skip it: an opt-out would be
        # taken by exactly the agent it exists to catch.
        if ticket.fields.get("gates", "fast") == "fast":
            cmd = str(ticket.fields.get("verify"))
            proc = subprocess.run(cmd, shell=True, cwd=str(REPO_ROOT))
            if proc.returncode != 0:
                raise BoardError(
                    f"independent re-run of the ticket verify failed "
                    f"(exit {proc.returncode}): {cmd}",
                    EXIT_REFUSED,
                )

        ticket.fields["status"] = "done"
        ticket.fields["commit"] = sha
        ticket.save()
        commit_board_change([ticket.path], f"board({ticket.id}): done {sha[:12]}")
        _regenerate_interfaces()

    payload = {"status": "done", "id": ticket.id, "commit": sha}
    _emit(payload, args.json, f"DONE {ticket.id} {sha[:12]}")
    return EXIT_OK


def _append_blocker(
    ticket: Ticket,
    block_class: str,
    failing_command: str,
    assertion: str,
    smallest_fix: str,
    log: str | None,
) -> None:
    stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    entry = [
        "",
        f"### Attempt {ticket.attempts} — {block_class} ({stamp})",
        "",
        f"- **Failing command:** `{failing_command}`",
        f"- **Assertion / contradiction:** {assertion}",
        f"- **Smallest change that would unblock:** {smallest_fix}",
    ]
    if log:
        entry.append(f"- **Log:** `{log}`")
    entry.append("")
    text = "\n".join(entry)
    if "## Blocker log" in ticket.body:
        ticket.body = ticket.body.rstrip("\n") + "\n" + text
    else:
        ticket.body = ticket.body.rstrip("\n") + "\n\n## Blocker log\n" + text

    BLOCKED_LOG.parent.mkdir(parents=True, exist_ok=True)
    with open(BLOCKED_LOG, "a", encoding="utf-8") as handle:
        handle.write(
            f"{stamp}\t{ticket.id}\tattempt={ticket.attempts}\tclass={block_class}\t{assertion}\n"
        )


def cmd_block(args: argparse.Namespace) -> int:
    if args.block_class not in BLOCK_CLASSES:
        raise BoardError(
            f"unknown class '{args.block_class}'; expected one of {BLOCK_CLASSES}",
            EXIT_USAGE,
        )
    for name, value in (
        ("--failing-command", args.failing_command),
        ("--assertion", args.assertion),
        ("--smallest-fix", args.smallest_fix),
    ):
        if not value or len(value.strip()) < 12:
            raise BoardError(
                f"{name} is required and must be substantive (>=12 chars). "
                "A blocker note a human cannot act on is worse than no note.",
                EXIT_USAGE,
            )

    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status not in ("in_progress", "todo"):
            raise BoardError(
                f"{ticket.id} is '{ticket.status}'; nothing to block", EXIT_REFUSED
            )

        dirty = working_tree_dirty()
        if dirty:
            raise BoardError(
                "working tree is dirty; commit the WIP on the ticket branch first:\n"
                + "\n".join(dirty),
                EXIT_PRECONDITION,
            )
        if current_branch() != "main":
            git("switch", "main")

        # Reload after the branch switch: main's copy of the ticket is the one to edit.
        ticket = find_ticket(args.id)
        refunded = args.block_class in REFUNDING_CLASSES
        _append_blocker(
            ticket,
            block_class=args.block_class,
            failing_command=args.failing_command,
            assertion=args.assertion,
            smallest_fix=args.smallest_fix,
            log=args.log,
        )
        if refunded:
            # An absent Docker daemon is not the ticket's fault. Blocking it here
            # would let one environment failure take out every integration ticket.
            ticket.fields["status"] = "todo"
            ticket.fields["attempts"] = max(0, ticket.attempts - 1)
            ticket.fields["branch"] = ""
        else:
            ticket.fields["status"] = "blocked"
            if args.branch:
                ticket.fields["branch"] = args.branch
        ticket.save()
        commit_board_change(
            [ticket.path],
            f"board({ticket.id}): {'released' if refunded else 'blocked'} {args.block_class}",
        )

    payload = {
        "status": "todo" if refunded else "blocked",
        "id": ticket.id,
        "class": args.block_class,
        "attempt_refunded": refunded,
    }
    _emit(
        payload,
        args.json,
        f"{'RELEASED' if refunded else 'BLOCKED'} {ticket.id} ({args.block_class})",
    )
    return EXIT_OK


def cmd_unblock(args: argparse.Namespace) -> int:
    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status != "blocked":
            raise BoardError(f"{ticket.id} is '{ticket.status}', not blocked", EXIT_REFUSED)
        ticket.fields["status"] = "todo"
        ticket.fields["branch"] = ""
        if args.reset_attempts:
            ticket.fields["attempts"] = 0
        ticket.save()
        commit_board_change([ticket.path], f"board({ticket.id}): unblocked")
    _emit({"status": "todo", "id": ticket.id}, args.json, f"UNBLOCKED {ticket.id}")
    return EXIT_OK


def cmd_reset_attempts(args: argparse.Namespace) -> int:
    """Refund the attempts of a `todo` ticket that starved on harness faults.

    `unblock` only accepts a `blocked` ticket, so a ticket whose attempts were
    consumed by RELEASES rather than blocks -- a killed driver, a branch collision,
    a permission gap -- lands in `todo` at MAX_ATTEMPTS and becomes permanently
    unselectable, with no verb able to recover it. `next` then reports it `starved`
    and the whole board stalls behind it. ER-018 hit this after three consecutive
    harness faults, none of them its own fault.

    This is a human action and is deliberately not in the agent's permitted set:
    refunding your own attempts would defeat the attempt cap entirely.
    """
    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status != "todo":
            raise BoardError(
                f"{ticket.id} is '{ticket.status}', not todo; "
                "use 'unblock' for a blocked ticket or 'release' for an in_progress one",
                EXIT_REFUSED,
            )
        if ticket.attempts == 0:
            raise BoardError(f"{ticket.id} already has 0 attempts", EXIT_REFUSED)
        before = ticket.attempts
        ticket.fields["attempts"] = 0
        ticket.fields["branch"] = ""
        ticket.save()
        commit_board_change(
            [ticket.path], f"board({ticket.id}): attempts reset {before} -> 0 ({args.reason})"
        )
    _emit(
        {"status": "todo", "id": ticket.id, "attempts": 0, "was": before},
        args.json,
        f"RESET {ticket.id} attempts {before} -> 0",
    )
    return EXIT_OK


def cmd_release(args: argparse.Namespace) -> int:
    """Return an interrupted in_progress ticket to todo without consuming state."""
    with BoardLock():
        ticket = find_ticket(args.id)
        if ticket.status != "in_progress":
            raise BoardError(
                f"{ticket.id} is '{ticket.status}', not in_progress", EXIT_REFUSED
            )
        ticket.fields["status"] = "todo"
        ticket.fields["branch"] = ""
        ticket.save()
        commit_board_change([ticket.path], f"board({ticket.id}): released")
    _emit({"status": "todo", "id": ticket.id}, args.json, f"RELEASED {ticket.id}")
    return EXIT_OK


# --------------------------------------------------------------------------
# plan-check
# --------------------------------------------------------------------------


def _acceptance_criteria(ticket: Ticket) -> list[str]:
    return re.findall(r"^- \[[ x]\] (AC\d+):", ticket.body, re.M)


def cmd_plan_check(args: argparse.Namespace) -> int:
    plan_path = Path(args.plan)
    if not plan_path.exists():
        raise BoardError(f"no plan at {plan_path}", EXIT_USAGE)
    try:
        plan = json.loads(plan_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise BoardError(f"{plan_path} is not valid JSON: {exc}", EXIT_USAGE)

    ticket = find_ticket(args.ticket)
    tickets = load_board()
    problems: list[str] = []

    if plan.get("ticket") != ticket.id:
        problems.append(f"plan.ticket is '{plan.get('ticket')}', expected '{ticket.id}'")

    files = plan.get("files") or []
    if not files:
        problems.append("plan.files is empty")
    tests = plan.get("tests") or []
    if not tests and ticket.fields.get("kind", "code") == "code":
        problems.append("plan.tests is empty; every code ticket must name its test files")

    criteria = set(_acceptance_criteria(ticket))
    if not criteria:
        problems.append("ticket has no numbered acceptance criteria (AC1, AC2, ...)")
    covered: set[str] = set()
    for entry in files:
        covered.update(entry.get("covers") or [])
    uncovered = sorted(criteria - covered)
    if uncovered:
        problems.append(
            "no planned file covers: " + ", ".join(uncovered)
            + " (add them to a file's 'covers' list, or the plan is incomplete)"
        )
    unknown = sorted(covered - criteria)
    if unknown:
        problems.append("plan covers unknown criteria: " + ", ".join(unknown))

    # Protected paths: the whole point of a ticket like "make this failing test
    # pass" is that the test does not change. plan-check is the first of two
    # enforcement points; gates.sh gate 0 is the second.
    protected = set(ticket.fields.get("protected_paths", []))
    allowed_extra = set(ticket.fields.get("extra_paths", []))
    owns_index: dict[str, str] = {}
    for other in tickets:
        if other.id == ticket.id or other.status == "done":
            continue
        for owned in other.fields.get("owns", []):
            owns_index[owned] = other.id

    for entry in files:
        path = entry.get("path")
        if not path:
            problems.append("a plan file entry has no 'path'")
            continue
        if path in protected:
            problems.append(
                f"'{path}' is in protected_paths and must not be modified by this ticket"
            )
        # No exemption for the ticket's own file. It is the live source of the
        # verify command, `protected_paths`, the gate scope and the acceptance
        # criteria, so letting a ticket plan an edit to itself would let the agent
        # rewrite its own grade. board.py owns every field; nothing here is
        # hand-editable during an iteration.
        if path.startswith("docs/implementation/"):
            problems.append(
                f"'{path}': tickets are written by board.py, not by hand -- "
                "including this ticket's own file"
            )
        if path.startswith(".claude/") or path in (
            "scripts/board.py",
            "scripts/gates.sh",
            "scripts/run-loop.sh",
        ):
            problems.append(f"'{path}': the loop may not modify its own machinery")
        if path == "DesignDoc.md" and ticket.fields.get("kind") != "spec-amendment":
            problems.append(
                "'DesignDoc.md': only a ticket with kind: spec-amendment may edit the spec. "
                "If the spec is wrong, block with class spec_contradiction and propose wording."
            )
        if path.startswith(".github/") and path not in allowed_extra:
            problems.append(f"'{path}': add it to the ticket's extra_paths to touch CI")
        owner = owns_index.get(path)
        if owner:
            problems.append(f"'{path}' is owned by {owner}, which is not done yet")

    if problems:
        for problem in problems:
            print(f"PLAN-DEFECT {problem}", file=sys.stderr)
        return EXIT_REFUSED
    print(f"OK plan for {ticket.id}: {len(files)} file(s), {len(tests)} test file(s)")
    return EXIT_OK


# --------------------------------------------------------------------------
# reporting
# --------------------------------------------------------------------------


def cmd_status(args: argparse.Namespace) -> int:
    tickets = load_board()
    counts = {status: 0 for status in STATUSES}
    for ticket in tickets:
        counts[ticket.status] = counts.get(ticket.status, 0) + 1
    ready = ready_tickets(tickets)
    by_milestone: dict[str, dict[str, int]] = {}
    for ticket in tickets:
        bucket = by_milestone.setdefault(
            str(ticket.fields.get("milestone", "?")), {s: 0 for s in STATUSES}
        )
        bucket[ticket.status] = bucket.get(ticket.status, 0) + 1

    payload = {
        "total": len(tickets),
        **counts,
        "ready": len(ready),
        "next": ready[0].id if ready else None,
        "blocked_ids": [t.id for t in tickets if t.status == "blocked"],
        "by_milestone": by_milestone,
    }
    if args.json:
        print(json.dumps(payload, indent=2))
    else:
        print(
            f"total={payload['total']} done={counts['done']} todo={counts['todo']} "
            f"in_progress={counts['in_progress']} blocked={counts['blocked']} "
            f"ready={payload['ready']}"
        )
        for milestone in sorted(by_milestone):
            bucket = by_milestone[milestone]
            print(
                f"  {milestone}: done={bucket['done']} todo={bucket['todo']} "
                f"blocked={bucket['blocked']}"
            )
        if payload["blocked_ids"]:
            print("  BLOCKED: " + ", ".join(payload["blocked_ids"]))
    return EXIT_OK


def cmd_show(args: argparse.Namespace) -> int:
    ticket = find_ticket(args.id)
    if args.json:
        print(json.dumps({**ticket.fields, "path": str(ticket.path.relative_to(REPO_ROOT))}, indent=2))
    else:
        print(ticket.path.read_text(encoding="utf-8"))
    return EXIT_OK


def cmd_verify_cmd(args: argparse.Namespace) -> int:
    ticket = find_ticket(args.id)
    print(ticket.fields.get("verify", ""))
    return EXIT_OK


def _regenerate_interfaces(commit: bool = True) -> None:
    """Rebuild INTERFACES.md from every done ticket's ``provides`` list.

    Each ticket runs in a fresh context, so nothing carries forward in memory.
    Without this file, ticket 60 has no idea ticket 13 already created
    ``ids.record_key()`` and the codebase grows three ULID helpers.
    """
    tickets = [t for t in load_board() if t.status == "done"]
    lines = [
        "# Interfaces provided by completed tickets",
        "",
        "Generated by `scripts/board.py`. Do not edit by hand.",
        "",
        "| Ticket | Provides |",
        "|---|---|",
    ]
    for ticket in sorted(tickets, key=lambda t: t.number):
        provides = ticket.fields.get("provides", [])
        if not provides:
            continue
        lines.append(f"| {ticket.id} | " + "<br>".join(f"`{p}`" for p in provides) + " |")
    INTERFACES_PATH.write_text("\n".join(lines) + "\n", encoding="utf-8")
    # Committed only as part of a `complete` transition, which requires a clean
    # tree afterwards. Run standalone this is a plain regeneration, and making a
    # commit the caller did not ask for would be surprising.
    if commit:
        commit_board_change([INTERFACES_PATH], "board: regenerate INTERFACES.md")


def cmd_overview(args: argparse.Namespace) -> int:
    """Regenerate BOARD.md from the tickets themselves.

    A hand-maintained overview drifts from the tickets within a day; generating it
    means the two can never disagree.
    """
    tickets = sorted(load_board(), key=lambda t: t.number)
    mark = {"done": "x", "in_progress": ">", "blocked": "!", "todo": " "}
    lines = [
        "# Implementation board",
        "",
        "Generated by `python3 scripts/board.py overview`. Do not edit by hand — "
        "the ticket files are the source of truth.",
        "",
        f"{sum(1 for t in tickets if t.status == 'done')} of {len(tickets)} done.",
        "",
    ]
    for milestone in sorted({str(t.fields.get("milestone", "?")) for t in tickets}):
        group = [t for t in tickets if str(t.fields.get("milestone")) == milestone]
        done = sum(1 for t in group if t.status == "done")
        lines += [
            f"## {milestone} — {done}/{len(group)}",
            "",
            "| | id | title | size | depends on | verify |",
            "|---|---|---|---|---|---|",
        ]
        for t in group:
            deps = ", ".join(t.depends_on) or "—"
            verify = str(t.fields.get("verify", "")).replace("|", "\\|")
            title = str(t.fields.get("title", "")).replace("|", "\\|")
            lines.append(
                f"| `{mark.get(t.status, '?')}` | {t.id} | {title} | "
                f"{t.fields.get('size')} | {deps} | `{verify}` |"
            )
        lines.append("")
    path = BOARD_DIR / "BOARD.md"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {path.relative_to(REPO_ROOT)} ({len(tickets)} tickets)")
    return EXIT_OK


def cmd_interfaces(args: argparse.Namespace) -> int:
    _regenerate_interfaces(commit=False)
    print(f"wrote {INTERFACES_PATH.relative_to(REPO_ROOT)} (not committed)")
    return EXIT_OK


def cmd_audit(args: argparse.Namespace) -> int:
    """Detect drift between the board and git."""
    tickets = load_board()
    drift: list[str] = []
    for ticket in tickets:
        if ticket.status == "done":
            sha = str(ticket.fields.get("commit", ""))
            if not sha:
                # Work completed before the loop existed (the spec amendment) has no
                # loop commit to point at. That is expected, not drift.
                if ticket.fields.get("kind") != "spec-amendment":
                    drift.append(f"{ticket.id}: done but has no commit")
            elif not commit_exists(sha):
                drift.append(f"{ticket.id}: commit {sha} is unreachable")
            elif not git_ok("merge-base", "--is-ancestor", sha, "main"):
                drift.append(f"{ticket.id}: commit {sha} is not reachable from main")
        if ticket.status == "in_progress":
            drift.append(f"{ticket.id}: left in_progress (run 'board.py release {ticket.id}')")
    if drift:
        for line in drift:
            print(f"DRIFT {line}", file=sys.stderr)
        return EXIT_REFUSED
    print("OK board and git agree")
    return EXIT_OK


# --------------------------------------------------------------------------
# entry point
# --------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="board.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)

    def add(name: str, func, help_text: str):
        p = sub.add_parser(name, help=help_text)
        p.set_defaults(func=func)
        p.add_argument("--json", action="store_true", help="emit machine-readable JSON")
        return p

    p = add("validate", cmd_validate, "check the whole board for defects")
    p.add_argument("board_dir", nargs="?", help="board directory (default docs/implementation)")
    p.add_argument("--strict", action="store_true", help="also check git reachability")

    add("next", cmd_next, "print the highest-priority READY ticket")

    p = add("claim", cmd_claim, "take a ticket in_progress and increment attempts")
    p.add_argument("id")
    p.add_argument("--session", help="Claude Code session id, for the audit trail")

    p = add("complete", cmd_complete, "mark a ticket done (requires a matching gate receipt)")
    p.add_argument("id")
    p.add_argument("--commit", required=True, help="the SHA of the work commit")

    p = add("block", cmd_block, "stop work on a ticket and record why")
    p.add_argument("id")
    p.add_argument("--class", dest="block_class", required=True, choices=BLOCK_CLASSES)
    p.add_argument("--failing-command", required=True)
    p.add_argument("--assertion", required=True)
    p.add_argument("--smallest-fix", required=True)
    p.add_argument("--log")
    p.add_argument("--branch")

    p = add("unblock", cmd_unblock, "return a blocked ticket to todo (human action)")
    p.add_argument("id")
    p.add_argument("--reset-attempts", action="store_true")

    p = add("release", cmd_release, "return an interrupted in_progress ticket to todo")
    p.add_argument("id")

    p = add(
        "reset-attempts",
        cmd_reset_attempts,
        "refund a todo ticket's attempts after harness faults starved it (human action)",
    )
    p.add_argument("id")
    p.add_argument("--reason", required=True, help="why the attempts were not the ticket's fault")

    p = add("plan-check", cmd_plan_check, "validate a change plan before any code is written")
    p.add_argument("plan")
    p.add_argument("--ticket", required=True)

    add("status", cmd_status, "summarise the board")

    p = add("show", cmd_show, "print one ticket")
    p.add_argument("id")

    p = add("verify-cmd", cmd_verify_cmd, "print a ticket's verify command")
    p.add_argument("id")

    add("overview", cmd_overview, "regenerate docs/implementation/BOARD.md")
    add("interfaces", cmd_interfaces, "regenerate docs/implementation/INTERFACES.md")
    add("audit", cmd_audit, "detect drift between the board and git")

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except BoardError as exc:
        print(f"ERROR {exc}", file=sys.stderr)
        return exc.code
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return EXIT_PRECONDITION


if __name__ == "__main__":
    sys.exit(main())
