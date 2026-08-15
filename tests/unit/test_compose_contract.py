"""The Compose substrate is asserted against DesignDoc.md and the S2.1 pin table.

S7.1 is the file every other layer stands on, and each of the six B1 defects it fixes
is invisible to a test that only checks the stack eventually came up: a missing
`ER_CONFIG`, a scheme on `ER_S3_ENDPOINT`, an absent artifacts mount and a floating
image tag all produce failures far from their cause, or -- worse -- a green run whose
artifacts never reached the host.

Two sources, deliberately. `docker compose config` is the rendered model, so it is
what proves the project name, the anchors and the interpolation defaults really
resolve; the raw document is what proves the `${ER_CPU_LIMIT:-2}` *expressions* are
still expressions, which rendering would have already flattened. Neither needs a
Docker daemon or a running service: this is the S8.1 unit layer, and `config` parses.
"""

from __future__ import annotations

import json
import re
import subprocess
from functools import cache
from pathlib import Path
from typing import Any

import yaml

from er.versions import IMAGE_PINS

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
COMPOSE_FILE = REPO_ROOT / "docker" / "compose.yaml"

# S7.4's prohibition, as a literal the grep below can look for. The one-shot
# initialisers make this flag's exit code a teardown signal rather than a test
# result, so it must appear in no file the substrate is driven from.
FORBIDDEN_INVOCATION = "--abort-on-container-exit"
SUBSTRATE_DIRECTORIES = ("docker", "scripts/ci")

# AC2: the values that are wrong in a way nothing downstream can recover from -- a
# scheme on the endpoint is rejected by httpfs, and a missing ER_CONFIG exits 2.
REQUIRED_ENV_VALUES = {
    "ER_CONFIG": "/app/configs/test.yaml",
    "ER_S3_ENDPOINT": "objectstore:9000",
    "ER_LAKE_DATA_PATH": "s3://lake/er/",
    "ER_LAKE_ALIAS": "lake",
    "ER_LAKE_METADATA_SCHEMA": "er_main",
}

# S7.1: MinIO refuses to start below these lengths, which is B1 defect #1.
MINIO_USER_MINIMUM = 3
MINIO_PASSWORD_MINIMUM = 8

SERVICES_UNDER_BOTH_PROFILES = ("pipeline", "benchmark")


def section(anchor: str) -> str:
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(f'<a id="{anchor}"></a>')
    assert start != -1, f"DesignDoc.md has no anchor {anchor}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


@cache
def compose_document() -> dict[str, Any]:
    """The file as authored, with anchors and merge keys resolved but nothing else.

    Interpolation has NOT happened here, which is the point: `${ER_CPU_LIMIT:-2}`
    reaches these assertions as the expression S10.2 requires it to still be.
    """
    assert COMPOSE_FILE.is_file(), "docker/compose.yaml does not exist"
    loaded = yaml.safe_load(COMPOSE_FILE.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict)
    return loaded


@cache
def rendered_config() -> dict[str, Any]:
    """`docker compose config` output -- the model Compose itself would act on.

    Both profiles are enabled, because a service behind a profile is absent from the
    rendered model and an assertion against an absent service asserts nothing.
    """
    command = [
        "docker",
        "compose",
        "-f",
        str(COMPOSE_FILE),
        "--profile",
        "test",
        "--profile",
        "bench",
        "config",
        "--format",
        "json",
    ]
    try:
        result = subprocess.run(command, capture_output=True, text=True, cwd=REPO_ROOT)
    except FileNotFoundError as exc:
        raise AssertionError(
            "the docker CLI is required to render docker/compose.yaml; "
            "`docker compose config` needs no daemon and no services (S8.1)"
        ) from exc
    assert result.returncode == 0, f"`docker compose config` failed:\n{result.stderr}"
    parsed = json.loads(result.stdout)
    assert isinstance(parsed, dict)
    return parsed


def rendered_service(name: str) -> dict[str, Any]:
    services = rendered_config()["services"]
    assert name in services, f"docker/compose.yaml declares no service {name}"
    service: dict[str, Any] = services[name]
    return service


def authored_service(name: str) -> dict[str, Any]:
    services = compose_document()["services"]
    assert name in services, f"docker/compose.yaml declares no service {name}"
    service: dict[str, Any] = services[name]
    return service


def cli_environment_names() -> frozenset[str]:
    """The complete `ER_*` set S6.1 says the CLI reads, parsed from the spec.

    Read rather than restated: S6.1 calls this set complete, so a variable added
    there and not to `x-er-env` has to fail here rather than at the first container
    that exits 2.
    """
    text = section("s6-1")
    start = text.find("The complete set the CLI reads:")
    assert start != -1, "S6.1 no longer enumerates the environment contract"
    end = text.find("\n\n", start)
    sentence = text[start:end] if end != -1 else text[start:]
    names = frozenset(re.findall(r"`(ER_[A-Z0-9_]+)`", sentence))
    assert len(names) >= len(REQUIRED_ENV_VALUES), f"S6.1 parsed to only {sorted(names)}"
    return names


def spec_resource_envelope() -> tuple[str, str]:
    """The `cpus` / `memory` expressions S7.1 compiles into `deploy.resources`."""
    match = re.search(r'cpus:\s*"([^"]+)",\s*memory:\s*"([^"]+)"', section("s7-1"))
    assert match, "S7.1 no longer states a deploy.resources.limits envelope"
    return match.group(1), match.group(2)


def test_project_name_image_and_pull_policy() -> None:
    # Without `name:` Compose derives the project from the directory -- `docker` --
    # and the image built as `er-pipeline:ci` under project `er` is never found.
    assert rendered_config()["name"] == "er"

    for name in SERVICES_UNDER_BOTH_PROFILES:
        service = rendered_service(name)
        assert service["image"] == "er-pipeline:ci", (
            f"{name} must consume the locally built tag, not a registry reference"
        )
        assert service["pull_policy"] == "never", (
            f"{name} must never attempt a registry pull for a local-only tag (S7.1)"
        )
        # From the authored document: rendering resolves `context` to an absolute
        # path, which says nothing about what the file asked for.
        assert authored_service(name)["build"] == {
            "context": "..",
            "dockerfile": "docker/Dockerfile",
        }, f"{name} must build the S7.3 image from the repository root as context"

    assert authored_service("pipeline")["profiles"] == ["test"]
    assert authored_service("benchmark")["profiles"] == ["bench"]


def test_pipeline_and_benchmark_carry_the_full_env() -> None:
    expected = cli_environment_names()

    environments = {}
    for name in SERVICES_UNDER_BOTH_PROFILES:
        environment = rendered_service(name)["environment"]
        present = {key: value for key, value in environment.items() if key.startswith("ER_")}
        assert frozenset(present) == expected, (
            f"{name}'s environment is not the S6.1 contract: "
            f"missing {sorted(expected - frozenset(present))}, "
            f"extra {sorted(frozenset(present) - expected)}"
        )
        empty = sorted(key for key, value in present.items() if not value)
        assert not empty, f"{name} carries {empty} as empty, which exits 2 at load (S6.1)"
        environments[name] = present

    # Every variable appears exactly once, in `x-er-env`, so the two services cannot
    # drift: `benchmark` adds BENCH_SCALE and nothing else (S7.1).
    assert environments["pipeline"] == environments["benchmark"]
    assert rendered_service("benchmark")["environment"]["BENCH_SCALE"] == "smoke"

    for key, value in REQUIRED_ENV_VALUES.items():
        assert environments["pipeline"][key] == value

    # Stated separately from the literal above because it is the reason for it:
    # DuckDB's httpfs rejects a URL in ENDPOINT, and the failure is at query time.
    assert "://" not in environments["pipeline"]["ER_S3_ENDPOINT"]


def test_image_digests_match_versions_module() -> None:
    for service, pin in IMAGE_PINS.items():
        image = rendered_service(service)["image"]
        assert image == pin.reference, (
            f"{service} runs {image}, not the S2.1 pin {pin.reference}; a comment "
            "asserting a digest ought to be there is not a pin"
        )
        assert "@sha256:" in image

    digest_pinned = {
        name: service["image"]
        for name, service in compose_document()["services"].items()
        if "@sha256:" in str(service.get("image", ""))
    }
    assert frozenset(digest_pinned) == frozenset(IMAGE_PINS), (
        "every digest-pinned service image needs a row in src/er/versions.py::IMAGE_PINS "
        f"(found {sorted(digest_pinned)})"
    )


def test_no_abort_on_container_exit_anywhere() -> None:
    offenders = []
    for directory in SUBSTRATE_DIRECTORIES:
        for path in sorted((REPO_ROOT / directory).rglob("*")):
            if not path.is_file():
                continue
            if FORBIDDEN_INVOCATION in path.read_text(encoding="utf-8"):
                offenders.append(str(path.relative_to(REPO_ROOT)))

    assert not offenders, (
        f"{offenders} drive the stack with {FORBIDDEN_INVOCATION}; the one-shot "
        "initialisers make that exit code a teardown signal, not a test result (S7.4)"
    )


def test_artifacts_bind_mount_and_resource_envelope() -> None:
    expected_cpus, expected_memory = spec_resource_envelope()

    for name in SERVICES_UNDER_BOTH_PROFILES:
        service = authored_service(name)
        assert "../artifacts:/app/artifacts" in service["volumes"], (
            f"{name} has no artifacts bind mount; report.py --compare and the CI "
            "artifact upload would read a file that cannot exist (M24)"
        )
        limits = service["deploy"]["resources"]["limits"]
        assert limits["cpus"] == expected_cpus
        assert limits["memory"] == expected_memory

    environment = compose_document()["x-er-env"]
    # S10.2: the two are written as one expression precisely so a run cannot be
    # given more cgroup quota than the threads DuckDB was told to plan against.
    assert environment["ER_DUCKDB_THREADS"] == expected_cpus
    assert environment["ER_DUCKDB_MEMORY_LIMIT"] == "${ER_DUCKDB_MEMORY_LIMIT:-4GB}"


def test_objectstore_credentials_and_readiness() -> None:
    server = authored_service("objectstore")
    init = authored_service("objectstore-init")

    user = server["environment"]["MINIO_ROOT_USER"]
    password = server["environment"]["MINIO_ROOT_PASSWORD"]
    assert len(user) >= MINIO_USER_MINIMUM and len(password) >= MINIO_PASSWORD_MINIMUM, (
        "MinIO refuses to start below the 3/8-character minimums (B1)"
    )
    assert init["environment"]["MINIO_ROOT_USER"] == user, (
        "the init container authenticates with the server's own credentials, or "
        "`mc alias set` fails and the bucket is never created (B1)"
    )
    assert init["environment"]["MINIO_ROOT_PASSWORD"] == password

    environment = compose_document()["x-er-env"]
    assert environment["ER_S3_ACCESS_KEY_ID"] == user
    assert environment["ER_S3_SECRET_ACCESS_KEY"] == password

    # A probe that can never pass is worse than none: the server image ships no `mc`
    # and no HTTP client, so anything gated on `service_healthy` here hangs forever.
    assert "healthcheck" not in server, "objectstore must declare no healthcheck (S7.1)"

    # Readiness is the bucket existing, which is what the init container completing
    # means. Until ER-020 lands `catalog-init`, both consumers gate on it directly.
    for name in SERVICES_UNDER_BOTH_PROFILES:
        depends_on = authored_service(name)["depends_on"]
        assert depends_on["objectstore-init"]["condition"] == "service_completed_successfully"
        assert depends_on["catalog"]["condition"] == "service_healthy"
