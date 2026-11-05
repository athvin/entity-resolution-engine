#!/usr/bin/env python3
"""Execute merge-bus.yml's steps the way Actions would, against a local fake origin.

Gives the workflow a real runner's semantics for the parts that carry bugs:
per-step `$GITHUB_OUTPUT`, `${{ }}` resolution in `env:`, `if:` gating, and a
failed step halting the job. `gh` is the stub beside this file; git operations
are real.
"""
import os, re, subprocess, sys, tempfile, pathlib, yaml

HERE = pathlib.Path(__file__).parent


def resolve(expr, ctx):
    """Substitute ${{ ... }} occurrences inside a string."""
    def one(m):
        return str(evaluate(m.group(1).strip(), ctx))
    return re.sub(r"\$\{\{(.*?)\}\}", one, expr, flags=re.S)


def evaluate(src, ctx):
    """Evaluate the small expression subset these workflows use."""
    py = src
    py = re.sub(r"steps\.([\w-]+)\.outputs\.([\w-]+)",
                lambda m: repr(ctx["steps"].get(m.group(1), {}).get(m.group(2), "")), py)
    py = re.sub(r"inputs\.([\w-]+)",
                lambda m: repr(ctx["inputs"].get(m.group(1), "")), py)
    py = re.sub(r"vars\.([\w-]+)",
                lambda m: repr(ctx["vars"].get(m.group(1), "")), py)
    py = re.sub(r"github\.([\w.]+)",
                lambda m: repr(ctx["github"].get(m.group(1), "")), py)
    py = py.replace("&&", " and ").replace("||", " or ")
    py = re.sub(r"!(?=[\s'\w(])", " not ", py)
    try:
        out = eval(py, {"__builtins__": {}}, {})
    except Exception as e:                                    # pragma: no cover
        raise SystemExit(f"cannot evaluate {src!r} -> {py!r}: {e}")
    if isinstance(out, bool):
        return "true" if out else "false"
    return out


def truthy(src, ctx):
    return evaluate(src, ctx) in (True, "true")


def main():
    wf, sim = sys.argv[1], pathlib.Path(sys.argv[2])
    inputs = dict(p.split("=", 1) for p in sys.argv[3:])
    inputs.setdefault("dry_run", "false")
    inputs.setdefault("max_batch", "")

    d = yaml.safe_load(open(wf))
    job = d["jobs"]["bus"]

    ctx = {
        "steps": {},
        "inputs": {k: (True if v == "true" else False if v == "false" else v)
                   for k, v in inputs.items()},
        "vars": {"MERGE_BUS_MAX_BATCH": os.environ.get("SIM_MAX_BATCH", "")},
        "github": {"run_id": "999", "token": "x",
                   "repository": "o/r", "server_url": "https://github.com"},
    }

    base_env = dict(os.environ)
    base_env.update({
        "PATH": f"{HERE}:{os.environ['PATH']}",
        "SIM": str(sim),
        "GITHUB_REPOSITORY": "o/r",
        "GITHUB_RUN_ID": "999",
        "GH_TOKEN": "x",
    })

    for i, step in enumerate(job["steps"]):
        name = step.get("name") or step.get("id") or step.get("uses") or f"step{i}"
        if "run" not in step:
            print(f"-- skip (action): {name}")
            continue
        cond = step.get("if")
        if cond and not truthy(cond.replace("${{", "").replace("}}", ""), ctx):
            print(f"-- skipped by if: {name}")
            continue

        env = dict(base_env)
        for k, v in (job.get("env") or {}).items():
            env[k] = resolve(str(v), ctx)
        for k, v in (step.get("env") or {}).items():
            env[k] = resolve(str(v), ctx)

        outfile = sim / f"out.{i}"
        outfile.write_text("")
        env["GITHUB_OUTPUT"] = str(outfile)

        print(f"\n===== STEP: {name} =====")
        p = subprocess.run(["bash", "-e", "-c", step["run"]],
                           env=env, cwd=sim / "landing",
                           capture_output=True, text=True)
        sys.stdout.write(p.stdout)
        if p.stderr.strip():
            sys.stdout.write("[stderr] " + p.stderr.rstrip().replace("\n", "\n[stderr] ") + "\n")

        outs = {}
        for line in outfile.read_text().splitlines():
            if "=" in line:
                k, v = line.split("=", 1)
                outs[k] = v
        if step.get("id"):
            ctx["steps"][step["id"]] = outs
        if outs:
            print(f"-- outputs: {outs}")

        if p.returncode != 0:
            print(f"\n!! step failed (exit {p.returncode}); job halts here, as Actions would")
            return p.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
