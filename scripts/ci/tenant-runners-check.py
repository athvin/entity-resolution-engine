#!/usr/bin/env python3
"""Check the shared Terraform/Helm tenant registry against rendered identities."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import yaml

CHART = Path("infra/k8s/charts/er-platform")


def errors_for(docs, orgs):
    expected = {"er-runner-" + hashlib.sha256(org.encode()).hexdigest()[:20] for org in orgs}
    accounts = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "ServiceAccount"}
    errors = []
    actual = {name for name in accounts if name.startswith("er-runner-")}
    if actual != expected:
        errors.append("rendered tenant accounts disagree with the registry")
    for name in expected | {"er-runner"}:
        if accounts.get(name, {}).get("automountServiceAccountToken") is not False:
            errors.append(f"{name} must not automount Kubernetes credentials")
    dispatchers = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "er-dispatcher"]
    if len(dispatchers) != 1:
        errors.append("expected one dispatcher")
    else:
        env = dispatchers[0]["spec"]["template"]["spec"]["containers"][0]["env"]
        enrollment = [v.get("value") for v in env if v["name"] == "ER_TENANT_RUNNER_ORGS"]
        if len(enrollment) != 1 or json.loads(enrollment[0]) != sorted(orgs):
            errors.append("dispatcher enrollment disagrees with the registry")
    for doc in docs:
        if doc.get("kind") in ("RoleBinding", "ClusterRoleBinding"):
            if any(s.get("kind") == "ServiceAccount" and s.get("name") in expected | {"er-runner"} for s in doc.get("subjects", [])):
                errors.append("a runner has Kubernetes RBAC")
    return errors


def main():
    registry = json.loads((CHART / "tenant-runners.json").read_text())
    if set(registry) != {"dev", "prod"}:
        raise ValueError("registry environments must be dev and prod")
    profiles = [(env, ns, orgs) for env, namespaces in registry.items() for ns, orgs in namespaces.items()]
    profiles += [("kind", "ci", {}), ("dev", "unenrolled-namespace", {})]
    for env, namespace, orgs in profiles:
        rendered = subprocess.check_output([
            "helm", "template", "er-platform", str(CHART), "--namespace", namespace,
            "--set", f"environment={env}", "--set", "efs.fileSystemId=fs-0",
            "--set", "efs.accessPointId=fsap-0",
        ], text=True)
        docs = [d for d in yaml.safe_load_all(rendered) if d]
        errors = errors_for(docs, orgs)
        if errors:
            raise ValueError(f"{env}/{namespace}: {errors}")
        print(f"tenant runners: {env}/{namespace}: {len(orgs)} enrolled; no runner Kubernetes RBAC")
    return 0


if __name__ == "__main__":
    sys.exit(main())
