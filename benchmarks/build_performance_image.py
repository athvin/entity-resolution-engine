"""Build small immutable source overlays on an existing pinned dependency image."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

PATHS = ("src", "dbt", "configs", "benchmarks", "fixtures", "tests", "scripts")
EXCLUDED = {"__pycache__", "dbt_packages", "target", "logs"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", type=Path, required=True)
    parser.add_argument("--base-image", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.context = args.context.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    files = {
        str(path.relative_to(args.context)): hashlib.sha256(path.read_bytes()).hexdigest()
        for name in PATHS
        for path in (args.context / name).rglob("*")
        if path.is_file()
        and not EXCLUDED.intersection(path.relative_to(args.context).parts)
        and path.name != ".user.yml"
        and path.suffix not in (".pyc", ".pyo")
    }
    for name in ("pyproject.toml", "uv.lock"):
        files[name] = hashlib.sha256((args.context / name).read_bytes()).hexdigest()
    source_sha = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    base_digest = subprocess.check_output(
        ["docker", "image", "inspect", args.base_image, "--format", "{{.Id}}"], text=True
    ).strip()
    base_tag = "er-perf-deps:" + base_digest.removeprefix("sha256:")[:16]
    subprocess.run(["docker", "tag", base_digest, base_tag], check=True)
    # Verify dependencies in the inherited image, and regenerate the console entry
    # point exactly as the project's updated project.scripts declaration specifies.
    install = (
        "import importlib.metadata as m,tomllib,pathlib; "
        "d=tomllib.loads(pathlib.Path('/app/pyproject.toml').read_text()); "
        "pins=[s.split('==') for s in d['project']['dependencies']]; "
        "assert all(m.version(n.split('[')[0])==v for n,v in pins); "
        "entry=d['project']['scripts']['er']; module,name=entry.split(':'); "
        "p=pathlib.Path('/app/.venv/bin/er'); "
        "p.write_text('#!/app/.venv/bin/python\\nfrom '+module+' import '+name"
        "+'\\n'+name+'()\\n'); "
        "p.chmod(0o755)"
    )
    dockerfile = args.out / "Dockerfile"
    dockerfile.write_text(
        f"FROM {base_tag}\nWORKDIR /app\n"
        + "\n".join(f"COPY {name}/ {name}/" for name in PATHS)
        + "\nCOPY pyproject.toml uv.lock ./\n"
        + f'LABEL er.source_sha256="{source_sha}"\n'
        + "RUN "
        + json.dumps(["python", "-c", install])
        + "\n"
        + "RUN dbt deps --project-dir dbt && rm -rf dbt/logs dbt/profiles/.user.yml\n"
    )
    with (args.out / "build.log").open("w") as log:
        subprocess.run(
            ["docker", "build", "-f", str(dockerfile.resolve()), "-t", args.tag, str(args.context)],
            stdout=log,
            stderr=subprocess.STDOUT,
            check=True,
        )
    digest = subprocess.check_output(
        ["docker", "image", "inspect", args.tag, "--format", "{{.Id}}"], text=True
    ).strip()
    (args.out / "source.json").write_text(
        json.dumps(
            {
                "source_sha256": source_sha,
                "image_digest": digest,
                "base_digest": base_digest,
                "files": files,
            },
            indent=2,
        )
    )
    print(json.dumps({"image": args.tag, "digest": digest, "source_sha256": source_sha}))


if __name__ == "__main__":
    main()
