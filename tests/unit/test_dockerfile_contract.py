"""The Dockerfile is asserted against DesignDoc.md and the S2.1 pin table.

S7.3 draws the image the whole Compose stack runs, and several of its properties are
load-bearing far from the file itself: the base image tags ARE the S2.1 Python and
`uv` rows (M25); the dev dependency group MUST be installed because the `pipeline`
service's command is `pytest`; and the extensions MUST arrive from the builder's
baked directory rather than from the network (B1, S4.0b).

Nothing here needs a Docker daemon. The build is the ticket's verify command; these
tests are what stops a green build from hiding a Dockerfile that drifted from the
spec it implements -- an image built on `python:3.13-slim` would still build.
"""

from __future__ import annotations

import re
from pathlib import Path

from er.versions import PINS

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
DOCKERFILE = REPO_ROOT / "docker" / "Dockerfile"
DOCKERIGNORE = REPO_ROOT / ".dockerignore"

# S7.3: two stages, in this order, and no third.
EXPECTED_STAGES = ("builder", "runtime")

# AC8's list. Each entry is something a COPY could otherwise carry into the image.
REQUIRED_DOCKERIGNORE_ENTRIES = (
    ".git",
    ".venv",
    ".loop",
    "artifacts",
    "dbt/target",
    "dbt/dbt_packages",
)

FROM_RE = re.compile(r"^FROM\s+(\S+)\s+AS\s+(\S+)\s*$", re.MULTILINE)
UV_IMAGE_RE = re.compile(r"^COPY\s+--from=(ghcr\.io/astral-sh/uv:\S+)\s", re.MULTILINE)
UV_SYNC_RE = re.compile(r"^RUN\s+uv sync[^\n]*", re.MULTILINE)


def dockerfile_text() -> str:
    assert DOCKERFILE.is_file(), "docker/Dockerfile does not exist"
    return DOCKERFILE.read_text(encoding="utf-8")


def stage_body(name: str) -> str:
    """Everything between `FROM … AS <name>` and the next FROM."""
    text = dockerfile_text()
    match = re.search(rf"^FROM\s+\S+\s+AS\s+{re.escape(name)}\s*$", text, re.MULTILINE)
    assert match, f"docker/Dockerfile has no stage named {name}"
    rest = text[match.end() :]
    following = re.search(r"^FROM\s", rest, re.MULTILINE)
    return rest[: following.start()] if following else rest


def section(anchor: str) -> str:
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(f'<a id="{anchor}"></a>')
    assert start != -1, f"DesignDoc.md has no anchor {anchor}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


def baked_extension_directory() -> str:
    """The directory S4.0b's `SET extension_directory` names.

    Read from the spec rather than restated here: the runtime connection and the
    build-time bake have to agree on this path, and the spec is where that agreement
    is defined.
    """
    match = re.search(r"SET extension_directory = '([^']+)'", section("s4-0b"))
    assert match, "S4.0b no longer names an extension_directory"
    return match.group(1)


def dockerignore_entries() -> set[str]:
    assert DOCKERIGNORE.is_file(), ".dockerignore does not exist"
    return {
        line.strip()
        for line in DOCKERIGNORE.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }


def test_two_stages_and_pinned_base_images() -> None:
    text = dockerfile_text()

    stages = FROM_RE.findall(text)
    assert tuple(name for _, name in stages) == EXPECTED_STAGES
    assert len(re.findall(r"^FROM\s", text, re.MULTILINE)) == len(stages), (
        "every FROM must name a stage, or the two-stage contract is not checkable"
    )

    expected_python = f"python:{PINS['python'].version}-slim"
    for image, name in stages:
        assert image == expected_python, (
            f"stage {name} is built on {image}, not the S2.1 Python pin {expected_python}"
        )

    assert UV_IMAGE_RE.findall(text) == [f"ghcr.io/astral-sh/uv:{PINS['uv'].version}"], (
        "the uv binary must come from the S2.1 uv pin, by tag -- never :latest"
    )


def test_builder_syncs_twice_and_never_uses_no_dev() -> None:
    text = dockerfile_text()
    assert "--no-dev" not in text, (
        "the pipeline service's command is pytest; --no-dev would build an image that "
        "cannot run the suite it exists to run (S7.3)"
    )

    builder = stage_body("builder")
    syncs = list(UV_SYNC_RE.finditer(builder))
    assert len(syncs) == 2, (
        "the builder syncs twice on purpose: dependencies first, then the project "
        f"(found {len(syncs)} `uv sync` invocations)"
    )

    first, second = syncs
    assert "--frozen" in first.group(0) and "--no-install-project" in first.group(0), (
        "the first sync installs dependencies only; src/ is not in the context yet"
    )
    assert second.group(0).split() == ["RUN", "uv", "sync", "--frozen"], (
        f"the second sync must be a plain `uv sync --frozen`, got {second.group(0)!r}"
    )

    copy_src = re.search(r"^COPY\s+src/\s+src/\s*$", builder, re.MULTILINE)
    assert copy_src, "the builder must COPY src/ before it installs the project"
    assert first.end() < copy_src.start() < second.start(), (
        "COPY src/ must sit between the two syncs, or the dependency layer is not cached"
    )


def test_runtime_copies_extension_dir_and_scripts() -> None:
    baked = baked_extension_directory()
    builder = stage_body("builder")
    runtime = stage_body("runtime")

    assert f"DUCKDB_EXTENSION_DIRECTORY={baked}" in builder, (
        f"the builder must bake into {baked}, the directory S4.0b's SET names"
    )
    assert f"COPY --from=builder {baked} {baked}" in runtime, (
        "the runtime stage must carry the baked directory, or autoinstall=false has "
        "nothing to load from (B1)"
    )
    assert "COPY --from=builder /app/.venv /app/.venv" in runtime, (
        "the runtime stage must carry the venv the builder synced"
    )
    assert re.search(r"^COPY\s+scripts/\s+scripts/\s*$", runtime, re.MULTILINE), (
        "the verify command runs /app/scripts/check_extensions.py, so scripts/ must ship"
    )


def test_dockerignore_excludes_build_noise() -> None:
    entries = dockerignore_entries()

    missing = [entry for entry in REQUIRED_DOCKERIGNORE_ENTRIES if entry not in entries]
    assert not missing, f".dockerignore does not exclude: {missing}"

    reincluded = [f"!{entry}" for entry in REQUIRED_DOCKERIGNORE_ENTRIES if f"!{entry}" in entries]
    assert not reincluded, f".dockerignore re-includes what it excluded: {reincluded}"

    # pyproject.toml declares `license = { file = "LICENSE" }`, so an excluded LICENSE
    # fails the builder's second `uv sync` at metadata generation, not at COPY.
    for required in ("LICENSE", "pyproject.toml", "uv.lock"):
        assert required not in entries, f".dockerignore excludes {required}; the build needs it"
