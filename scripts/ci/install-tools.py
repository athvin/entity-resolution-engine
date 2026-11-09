#!/usr/bin/env python3
"""Install pinned CI tools; validate cached archives before executing any bytes."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile


def install(name, spec, cache, destination):
    cache.mkdir(parents=True, exist_ok=True)
    destination.mkdir(parents=True, exist_ok=True)
    archive = cache / (spec["sha256"] + ".tar.gz")
    if not archive.exists():
        with tempfile.TemporaryDirectory(dir=cache) as temp:
            download = Path(temp) / "download"
            subprocess.run([
                "curl", "--fail", "--location", "--silent", "--show-error",
                "--connect-timeout", "10", "--max-time", "60", "--retry", "3",
                "--retry-all-errors", "--retry-max-time", "180",
                "--output", str(download), spec["url"],
            ], check=True)
            verify(download, spec["sha256"])
            download.replace(archive)
    verify(archive, spec["sha256"])
    # Extract ONLY the named regular file. Never trust archive paths or links.
    with tarfile.open(archive) as bundle:
        member = bundle.getmember(spec["member"])
        if not member.isfile():
            raise ValueError(f"{name}: expected a regular file")
        with bundle.extractfile(member) as source, (destination / name).open("wb") as out:
            shutil.copyfileobj(source, out)
    (destination / name).chmod(0o755)
    if name == "mold":
        (destination / "ld.mold").unlink(missing_ok=True)
        (destination / "ld.mold").symlink_to("mold")
    print(f"{name} {spec['version']}: checksum verified", flush=True)


def verify(path, expected):
    with path.open("rb") as stream:
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
        actual = digest.hexdigest()
    if actual != expected:
        raise ValueError(f"checksum mismatch: {path.name}; refusing to install")


def main():
    if (platform.system(), platform.machine()) != ("Linux", "x86_64"):
        raise ValueError("CI tool pins support Linux x86_64 only")
    specs = json.loads(Path(__file__).with_name("tools.json").read_text())
    root = Path(os.environ.get("CI_TOOLS_CACHE", str(Path.home() / ".cache/er-ci-tools")))
    for name in sys.argv[1:]:
        install(name, specs[name], root / "archives", root / "bin")
    with open(os.environ["GITHUB_PATH"], "a") as output:
        print(root / "bin", file=output)


if __name__ == "__main__":
    main()
