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

from er.versions import GO_BUILD_IMAGE, PINS, SOURCE_PINS

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
DOCKERFILE = REPO_ROOT / "docker" / "Dockerfile"
DOCKERIGNORE = REPO_ROOT / ".dockerignore"

# infrastructure.md S11's two new images: the control plane and the web UI.
API_DOCKERFILE = REPO_ROOT / "docker" / "Dockerfile.api"
WEB_DOCKERFILE = REPO_ROOT / "frontend" / "Dockerfile"

# S7.3: compile storage tools, build Python, then assemble the shared runtime.
EXPECTED_STAGES = ("storage-builder", "builder", "runtime")

# AC8's list. Each entry is something a COPY could otherwise carry into the image.
REQUIRED_DOCKERIGNORE_ENTRIES = (
    ".git",
    ".venv",
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


def test_build_stages_and_pinned_base_images() -> None:
    text = dockerfile_text()

    stages = FROM_RE.findall(text)
    assert tuple(name for _, name in stages) == EXPECTED_STAGES
    assert len(re.findall(r"^FROM\s", text, re.MULTILINE)) == len(stages), (
        "every FROM must name a stage, or the build contract is not checkable"
    )

    expected_python = f"python:{PINS['python'].version}-slim"
    for image, name in stages:
        expected = GO_BUILD_IMAGE.reference if name == "storage-builder" else expected_python
        assert image == expected, f"stage {name} is built on {image}, not the S2.1 pin {expected}"

    assert UV_IMAGE_RE.findall(text) == [f"ghcr.io/astral-sh/uv:{PINS['uv'].version}"], (
        "the uv binary must come from the S2.1 uv pin, by tag -- never :latest"
    )


def test_storage_build_verifies_sources_and_bundles_both_tools() -> None:
    builder = stage_body("storage-builder")
    runtime = stage_body("runtime")
    archives = re.findall(r"^ADD --checksum=sha256:([0-9a-f]{64}) (\S+) ", builder, re.MULTILINE)
    assert set(archives) == {(pin.archive_sha256, pin.archive_url) for pin in SOURCE_PINS.values()}
    assert "GOTOOLCHAIN=local" in builder and "GOFLAGS=-mod=readonly" in builder
    assert "CGO_ENABLED=0" in builder
    for pin in SOURCE_PINS.values():
        binary = pin.repository.split("/")[-1]
        assert f"cmd.ReleaseTag={pin.release}" in builder
        assert f"cmd.CommitID={pin.commit}" in builder
        assert f"/out/{binary} --version" in builder
        assert f"/src/{binary}/LICENSE" in runtime and f"/src/{binary}/NOTICE" in runtime
    assert "COPY --from=storage-builder /out/minio /out/mc /usr/local/bin/" in runtime


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


def file_text(path: Path) -> str:
    assert path.is_file(), f"{path.relative_to(REPO_ROOT)} does not exist"
    return path.read_text(encoding="utf-8")


def stage_body_of(text: str, name: str) -> str:
    """Everything between `FROM … AS <name>` and the next FROM, for any Dockerfile."""
    match = re.search(rf"^FROM\s+\S+\s+AS\s+{re.escape(name)}\s*$", text, re.MULTILINE)
    assert match, f"no stage named {name}"
    rest = text[match.end() :]
    following = re.search(r"^FROM\s", rest, re.MULTILINE)
    return rest[: following.start()] if following else rest


def effective_user(text: str) -> str:
    """The user the image runs as: the last USER instruction of the final stage.

    Asserted on the *last* instruction deliberately -- a `USER er` followed by a
    `USER root` still greps as non-root while running everything as root.
    """
    users = re.findall(r"^USER\s+(\S+)\s*$", text, re.MULTILINE)
    assert users, "no USER instruction; the image runs as root (infrastructure.md S5.6)"
    return users[-1]


def test_all_three_images_run_as_non_root() -> None:
    """infrastructure.md S5.6: Pod Security `restricted` requires a non-root USER,
    and a root engine process in a PII cluster is unforced risk. One loop, because
    the requirement is one rule over all three images, not three conventions."""
    for dockerfile in (DOCKERFILE, API_DOCKERFILE, WEB_DOCKERFILE):
        user = effective_user(file_text(dockerfile))
        assert user not in {"root", "0"}, f"{dockerfile.name} runs as {user}"


def test_pipeline_runtime_owns_every_directory_it_writes() -> None:
    """A.5's whole risk: a USER added without ownership turns the first write --
    MinIO's /data, DuckDB's spill, dbt's logs -- into EACCES far from this file.
    The anonymous `/data` volume inherits the image directory's ownership, which
    is why it must exist here and not only at `minio server /data` start."""
    runtime = stage_body("runtime")

    for directory in ("/app/artifacts", "/app/.bench", "/app/.tmp", "/app/dbt/.tmp", "/data"):
        assert re.search(rf"^RUN mkdir -p .*{re.escape(directory)}\b", runtime, re.MULTILINE), (
            f"the runtime stage must create {directory} before USER drops privileges"
        )
        assert re.search(rf"chown [\w:]+ .*{re.escape(directory)}\b", runtime), (
            f"{directory} must belong to the runtime user, or the first write fails"
        )
    # dbt/ is the one COPYed tree the runtime user writes into (target/, logs/).
    assert re.search(r"^COPY --chown=\S+ dbt/ dbt/\s*$", runtime, re.MULTILINE)


def test_api_dockerfile_is_pinned_and_slim() -> None:
    """S11's er-api row: `src/er` + `server/src/erserver`, no dbt project, no MinIO,
    no tests -- under the same S2.1 base-image pins as docker/Dockerfile."""
    text = file_text(API_DOCKERFILE)

    stages = FROM_RE.findall(text)
    assert tuple(name for _, name in stages) == ("builder", "runtime")
    assert len(re.findall(r"^FROM\s", text, re.MULTILINE)) == len(stages)
    expected_python = f"python:{PINS['python'].version}-slim"
    for image, name in stages:
        assert image == expected_python, f"stage {name} is built on {image}, not {expected_python}"
    assert UV_IMAGE_RE.findall(text) == [f"ghcr.io/astral-sh/uv:{PINS['uv'].version}"]

    runtime = stage_body_of(text, "runtime")
    assert "COPY --from=builder /app/server/.venv /app/server/.venv" in runtime
    assert f"COPY --from=builder {baked_extension_directory()} {baked_extension_directory()}" in (
        runtime
    ), "the API's read path ATTACHes DuckLake under autoinstall=false, like the pipeline (B1)"
    for tree in ("src/", "server/src/"):
        assert re.search(rf"^COPY\s+{re.escape(tree)}\s+{re.escape(tree)}\s*$", runtime, re.M), (
            f"the runtime stage must carry {tree}; both installs are editable"
        )

    # Slim is a list of absences (S11): what the pipeline image carries and this
    # one must not. `storage-builder` covers both MinIO binaries.
    for forbidden in ("storage-builder", "COPY dbt/", "COPY tests/", "COPY fixtures/", "dbt deps"):
        assert forbidden not in text, f"Dockerfile.api must not carry {forbidden!r}"


def test_web_dockerfile_is_digest_pinned_standalone_node() -> None:
    """S11's er-web row: Next standalone output on Node 22, with the base pinned by
    digest the way docker/Dockerfile pins its Go build image -- a tag a human reads,
    a digest that makes it immutable."""
    text = file_text(WEB_DOCKERFILE)

    images = {image for image, _ in FROM_RE.findall(text)}
    assert len(images) == 1, "builder and runtime must share one pinned Node base"
    (image,) = images
    assert re.fullmatch(r"node:22[\w.-]*@sha256:[0-9a-f]{64}", image), (
        f"the Node base must be node:22, pinned by digest; got {image}"
    )

    runtime = stage_body_of(text, "runtime")
    assert "/app/.next/standalone" in runtime, (
        "the runtime stage must deploy the standalone output, not node_modules"
    )
    assert "/app/.next/static" in runtime, (
        "server.js serves static assets only if they are copied in beside it"
    )
    assert '"server.js"' in runtime, "standalone output is started as `node server.js`"

    ignore = WEB_DOCKERFILE.parent / ".dockerignore"
    assert "node_modules" in file_text(ignore).split(), (
        "frontend/.dockerignore must exclude node_modules: a host install carries "
        "host-platform native binaries into a linux/amd64 image"
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
