"""Fleet status for a herdr session.

Runs `herdr agent list` in an explicit or inherited session and, for every agent other than the calling
pane, prints its reported status beside what its screen actually shows.

The two disagree in the case that matters: an agent that hit a rate limit, ran
out of context or had its goal paused is finished doing work but still reports
`idle`, because `idle` means "ready for input", not "done with its task". So a
screen line wins over a status field, and a known stuck-marker wins over both --
those are promoted to an ALERT column so a dead pane cannot read as a quiet one.
"""

import argparse
import json
import os
import re
import subprocess
import sys

# Markers that mean "this agent stopped for a reason you want to know about".
# Ordered: the first match is reported.
STUCK = (
    ("429 Too Many Requests", "RATE-LIMITED (429)"),
    ("exceeded retry limit", "RETRIES EXHAUSTED"),
    ("Too Many Requests", "RATE-LIMITED"),
    ("rate limit", "RATE-LIMITED"),
    ("Goal paused", "GOAL PAUSED"),
    ("usage limit", "USAGE LIMIT"),
    ("Context low", "CONTEXT LOW"),
    ("context left", "CONTEXT LOW"),
    ("No conversation found", "SESSION LOST"),
    ("Invalid API key", "AUTH FAILED"),
    ("authentication", "AUTH FAILED"),
)

# Chrome every agent TUI paints regardless of state. Never the answer to
# "what is this agent doing".
CHROME = (
    "Ask Codex to do anything",
    "Ask Claude",
    "for agents",
    "for shortcuts",
    "to view",
    "to interrupt",
    "to expand",
    "Show details",
    "Try again",
    "bypass permissions",
    "auto-accept",
)

# A status bar: "k3 max · ~/path · task · Main [default]".
STATUS_BAR = re.compile(r"·\s*~?/")
# A rule or spinner-only line: nothing but box drawing, dashes and spaces.
RULE = re.compile(r"^[\s─-╿⠀-⣿\-_=·•■□▪▶◀✳*]+$")
# The input prompt line.
PROMPT = re.compile(r"^[›❯>]\s")


def screen(command, pane):
    try:
        proc = subprocess.run(
            command + ["agent", "read", pane,
             "--source", "recent-unwrapped", "--lines", "60"],
            capture_output=True, text=True, timeout=20,
        )
    except subprocess.TimeoutExpired:
        return None, "<read timed out>"
    if proc.returncode != 0:
        return None, "<read failed: %s>" % proc.stderr.strip()[:60]

    lines = [ln.strip() for ln in proc.stdout.splitlines() if ln.strip()]

    alert = ""
    for needle, label in STUCK:
        if any(needle.lower() in ln.lower() for ln in lines):
            alert = label
            break

    substantive = [
        ln for ln in lines
        if not any(c in ln for c in CHROME)
        and not STATUS_BAR.search(ln)
        and not RULE.match(ln)
        and not PROMPT.match(ln)
        and len(ln) > 3
    ]
    return alert, (substantive[-1][:84] if substantive else "")


ROW = "%-8s %-7s %-8s %-10s %-18s %s"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", help="Named Herdr session, including default")
    args = parser.parse_args()
    inherited = os.environ.get("HERDR_PANE_ID")
    if not args.session and not inherited:
        parser.error("outside Herdr, select a session with --session")
    command = [os.environ.get("HERDR_BIN_PATH") or "herdr"]
    if args.session:
        command += ["--session", args.session]
    # A caller ID from a different session must not hide the selected agent.
    me = inherited if not args.session else None
    proc = subprocess.run(command + ["agent", "list"], capture_output=True,
                          text=True, timeout=20, check=True)
    agents = json.loads(proc.stdout)["result"]["agents"]

    print(ROW % ("PANE", "KIND", "STATUS", "NAME", "ALERT", "LAST LINE ON SCREEN"))
    stuck = 0
    for a in sorted(agents, key=lambda x: x["pane_id"]):
        pane = a["pane_id"]
        if pane == me:
            alert, last = "", "<- this agent"
        else:
            alert, last = screen(command, pane)
            if alert:
                stuck += 1
        print(ROW % (
            pane,
            a.get("agent", "?"),
            a.get("agent_status", "?"),
            (a.get("name") or "-")[:10],
            alert or "-",
            last,
        ))

    if stuck:
        print("\n%d of %d agents are stuck despite their reported status."
              % (stuck, sum(a["pane_id"] != me for a in agents)))


if __name__ == "__main__":
    main()
