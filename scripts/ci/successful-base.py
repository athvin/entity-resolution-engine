#!/usr/bin/env python3
"""Find a green ancestor for push selection; uncertainty means full CI."""
import json
import os
import re
import subprocess


def find_base(runs, head, current_run):
    for run in runs:
        sha = run["head_sha"]
        if (str(run["id"]) == current_run or run["conclusion"] != "success"
                or run["head_branch"] != "main" or run["event"] != "push"
                or not re.fullmatch(r"[a-f0-9]{40}", sha)):
            continue
        # Equality is valid: a re-run may verify an already green tree.
        if subprocess.run(["git", "merge-base", "--is-ancestor", sha, head],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            return sha
    return ""


def main():
    base = ""
    if os.environ.get("EVENT") == "push":
        try:
            repo = os.environ["GITHUB_REPOSITORY"]
            workflow = os.environ["CI_WORKFLOW"]
            data = json.loads(subprocess.check_output([
                "gh", "api", f"repos/{repo}/actions/workflows/{workflow}/runs?branch=main&event=push&status=success&per_page=100"
            ], text=True, timeout=30))
            base = find_base(data["workflow_runs"], os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"])
        except (KeyError, ValueError, subprocess.SubprocessError):
            print("::warning::cannot establish a green ancestor; running full CI")
    with open(os.environ["GITHUB_OUTPUT"], "a") as out:
        print(f"sha={base}", file=out)


if __name__ == "__main__":
    main()
