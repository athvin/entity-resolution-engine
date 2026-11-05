#!/usr/bin/env python3
"""Cross-chart selector gate for the two Helm charts.

`helm lint`, `helm template` and `kubeconform` all answer "is each document
well-formed". None of them answers the question that actually broke:

> does this selector match any pod that exists?

`allow-alb-to-web`'s `podSelector` said `app.kubernetes.io/name: er-web`, from
when the BFF was its own Deployment. After the pods were merged the only pod
carries `name: web-api`, so the policy matched NOTHING — and the namespace's
`default-deny-ingress` would have blackholed the ALB the first time
`ingress.enabled` was flipped on. Every document was valid YAML, valid against
the Kubernetes schema, and lint-clean. It had been wrong for weeks.

The two charts are why no single-chart check could see it: the policy is in
`er-dev-namespace` and the pod it selects is in `er-platform`. This renders both
and intersects them.

Generalised to the bug CLASS rather than the one instance, because the colour
dimension is about to add selectors that are supposed to resolve to different
pods and the same mistake would be invisible again:

  1. every Service selector matches at least one rendered pod
  2. every NetworkPolicy podSelector matches at least one rendered pod
  3. the ALB ingress hole does NOT open the control plane's port

(3) is the one that is a security property rather than a correctness one. §4.1
says "er-api is on no load balancer in any environment"; after the pod merge
that holds only because the hole is PORT-scoped, since the selector necessarily
matches a pod that also serves er-api on 8000. A reviewer widening the port list
would quietly publish the control plane, and nothing else in the tree would
notice.

Rename detection, the property every gate in this repo needs: if the render
produced no pods, no Services or no NetworkPolicies, this exits 2 rather than
reporting success. A gate that passes having examined nothing is worse than no
gate.
"""

from __future__ import annotations

import subprocess
import sys

try:
    import yaml
except ModuleNotFoundError:  # pragma: no cover - the workflow installs it
    print(
        "charts-check: PyYAML is required and was not importable.\n"
        "The workflow installs it explicitly rather than relying on the runner "
        "image, so this failing means that step is gone — fix it there. Do not "
        "let this pass as green.",
        file=sys.stderr,
    )
    raise SystemExit(2) from None

REPO_CHART = "infra/k8s/charts/er-platform"
NS_CHART = "infra/k8s/charts/er-dev-namespace"

# The namespace both renders agree on: er-dev-namespace CREATES it (installed
# into `default`), er-platform is installed INTO it.
NAMESPACE = "ci"


def render(args: list[str]) -> list[dict]:
    """`helm template` one chart into a list of manifests."""
    done = subprocess.run(
        ["helm", "template", *args], capture_output=True, text=True, check=False
    )
    if done.returncode != 0:
        print(f"charts-check: helm template failed\n{done.stderr}", file=sys.stderr)
        raise SystemExit(1)
    return [doc for doc in yaml.safe_load_all(done.stdout) if doc]


def pod_templates(docs: list[dict]) -> list[tuple[str, dict]]:
    """(name, labels) for every pod template in the render.

    Deployments and StatefulSets only: a Job's pod is not a Service backend, and
    a CronJob's is not either. Including them would let a selector "match" a pod
    that exists for ninety seconds a day.
    """
    out = []
    for doc in docs:
        if doc.get("kind") not in ("Deployment", "StatefulSet"):
            continue
        labels = doc["spec"]["template"]["metadata"].get("labels", {})
        out.append((doc["metadata"]["name"], labels))
    return out


def matches(selector: dict, labels: dict) -> bool:
    return all(labels.get(key) == value for key, value in selector.items())


def main() -> int:
    platform = render(
        [
            "er-platform",
            REPO_CHART,
            "--namespace",
            NAMESPACE,
            "--set",
            f"developer={NAMESPACE}",
            # Required to install; any syntactically valid value renders.
            "--set",
            "efs.fileSystemId=fs-0",
            "--set",
            "efs.accessPointId=fsap-0",
            # The edge is off by default. Render it ON: the whole point is to
            # check the objects that only exist once someone enables it, which
            # is exactly when nobody is looking at this file.
            "--set",
            "ingress.enabled=true",
            "--set",
            "ingress.certificateArn=arn:aws:acm:us-east-2:0:certificate/0",
        ]
    )
    namespace_chart = render(
        [
            NAMESPACE,
            NS_CHART,
            "--namespace",
            "default",
            "--set",
            f"developer={NAMESPACE}",
            # Same reasoning: the hole only renders when the edge is enabled,
            # and the two flags are meant to be flipped together.
            "--set",
            "webIngressFromVpc.enabled=true",
        ]
    )

    pods = pod_templates(platform)
    services = [d for d in platform if d.get("kind") == "Service"]
    policies = [d for d in namespace_chart if d.get("kind") == "NetworkPolicy"]

    print(f"charts-check: {len(pods)} pod template(s): {[n for n, _ in pods]}")
    print(f"charts-check: {len(services)} Service(s), {len(policies)} NetworkPolicy(s)")

    if not pods or not services or not policies:
        print(
            "charts-check: the render produced no pods, no Services or no "
            "NetworkPolicies — a chart or a template must have moved. Fix the "
            "render arguments in scripts/ci/charts-check.py; do not let this "
            "pass as green.",
            file=sys.stderr,
        )
        return 2

    problems: list[str] = []

    # (1) every Service selector resolves to a pod
    for svc in services:
        selector = svc["spec"].get("selector")
        name = svc["metadata"]["name"]
        if not selector:
            problems.append(f"Service/{name} has no selector")
            continue
        hit = [pod for pod, labels in pods if matches(selector, labels)]
        if not hit:
            problems.append(
                f"Service/{name} selector {selector} matches NO pod — it would "
                f"have no endpoints and answer connection-refused"
            )
        else:
            print(f"  Service/{name} -> {hit}")

    # (2) every NetworkPolicy podSelector resolves to a pod. An EMPTY selector
    # ({}) is the documented all-pods-in-this-namespace form that
    # `default-deny-ingress` uses deliberately, so it is not a finding.
    for pol in policies:
        name = pol["metadata"]["name"]
        selector = pol["spec"].get("podSelector", {})
        if not selector or not selector.get("matchLabels"):
            print(f"  NetworkPolicy/{name} -> all pods (empty selector, by design)")
            continue
        labels = selector["matchLabels"]
        hit = [pod for pod, pod_labels in pods if matches(labels, pod_labels)]
        if not hit:
            problems.append(
                f"NetworkPolicy/{name} podSelector {labels} matches NO pod — the "
                f"rule is inert, and with default-deny-ingress in the same "
                f"namespace whatever it was meant to admit is blackholed"
            )
        else:
            print(f"  NetworkPolicy/{name} -> {hit}")

    # (3) the ALB hole must not reach the control plane
    alb = next((p for p in policies if p["metadata"]["name"] == "allow-alb-to-web"), None)
    api = next((s for s in services if s["metadata"]["name"] == "er-api"), None)
    if alb is None or api is None:
        problems.append(
            "expected a NetworkPolicy/allow-alb-to-web and a Service/er-api to "
            "check the §4.1 boundary against; one is missing, so this check "
            "examined nothing"
        )
    else:
        open_ports = {
            port["port"]
            for rule in alb["spec"].get("ingress", [])
            for port in rule.get("ports", [])
        }
        api_ports = {port["port"] for port in api["spec"]["ports"]}
        print(f"  allow-alb-to-web opens {sorted(open_ports)}; er-api is on {sorted(api_ports)}")
        exposed = open_ports & api_ports
        if exposed:
            problems.append(
                f"the ALB ingress hole opens {sorted(exposed)}, which is er-api's "
                f"port: §4.1 says er-api is on no load balancer in ANY "
                f"environment, and after the pod merge only the PORT list keeps "
                f"that true"
            )

    if problems:
        print("\ncharts-check FAILED:\n", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print("\ncharts-check passed: every selector resolves, and the edge hole is web-only.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
