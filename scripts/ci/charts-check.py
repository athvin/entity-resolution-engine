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
            # The TargetGroupBinding only renders once an ARN is set (prod's
            # edge), so render it ON here — the same reason ingress is on
            # above: the check exists for the objects nobody sees until the
            # environment that needs them is live.
            "--set",
            "edge.targetGroupArn=arn:aws:elasticloadbalancing:us-east-2:0:targetgroup/t/0",
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

    # (3b) the TargetGroupBinding must reference the dedicated er-web-edge Service, by
    # name and port. Binding a preview Service would health-check the colour
    # not serving; a port the Service does not carry registers targets that
    # never go healthy, which is the edge-stuck-at-503 failure #556 exists to
    # close. Rendered on (above) precisely so this is checked here rather than
    # against a live cluster.
    tgb = next((d for d in platform if d.get("kind") == "TargetGroupBinding"), None)
    web = next((s for s in services if s["metadata"]["name"] == "er-web-edge"), None)
    if tgb is None or web is None:
        problems.append(
            "expected a TargetGroupBinding and a Service/er-web-edge to check the "
            "edge binding against; one is missing, so this check examined nothing"
        )
    else:
        ref = tgb["spec"]["serviceRef"]
        web_ports = {port["port"] for port in web["spec"]["ports"]}
        if ref["name"] != "er-web-edge":
            problems.append(
                f"TargetGroupBinding binds Service/{ref['name']}, not er-web-edge — "
                f"the edge Service stages registration before removing old targets"
            )
        if ref["port"] not in web_ports:
            problems.append(
                f"TargetGroupBinding port {ref['port']} is not one of er-web-edge's "
                f"ports {sorted(web_ports)} — targets would never health-check"
            )
        print(f"  TargetGroupBinding -> Service/{ref['name']}:{ref['port']}")

    # (4) the live pair and the preview pair must select DIFFERENT colours
    #
    # The one failure that would make the whole cutover decorative without
    # looking broken: a preview Service pointing at the colour already serving.
    # The deploy's smoke test runs against the previews, so if they aim at the
    # live pod it passes by testing production, every time, and the new colour
    # is promoted having been checked by nothing.
    #
    # The chart makes it unrepresentable today — `er-platform.inactiveColor` is
    # DERIVED from `color.active` rather than configured, and an invalid colour
    # is a `fail` at render. This is the belt: a hand-edit of services.yaml that
    # pasted the wrong helper call would restore the hazard, and nothing else
    # would notice.
    COLOR = "er.dupezero.com/color"
    live_pair = {"er-api", "er-web"}
    preview_pair = {"er-api-preview", "er-web-preview"}
    colour_of = {
        svc["metadata"]["name"]: (svc["spec"].get("selector") or {}).get(COLOR)
        for svc in services
    }
    present_live = live_pair & colour_of.keys()
    present_preview = preview_pair & colour_of.keys()

    if not present_live or not present_preview:
        problems.append(
            f"expected the live pair {sorted(live_pair)} and the preview pair "
            f"{sorted(preview_pair)} to render, got {sorted(colour_of)} — the "
            f"colour check examined nothing"
        )
    else:
        live_colours = {colour_of[n] for n in present_live}
        preview_colours = {colour_of[n] for n in present_preview}
        print(f"  live {sorted(present_live)} -> {sorted(live_colours)}; "
              f"preview {sorted(present_preview)} -> {sorted(preview_colours)}")
        if None in live_colours | preview_colours:
            problems.append(
                f"a Service selector carries no {COLOR}: live={sorted(live_colours)} "
                f"preview={sorted(preview_colours)}. Without it the Service selects "
                f"BOTH colours at once, so a cutover would never move traffic"
            )
        elif len(live_colours) != 1 or len(preview_colours) != 1:
            problems.append(
                f"the pairs disagree internally: live={sorted(live_colours)} "
                f"preview={sorted(preview_colours)}. er-api and er-web are the same "
                f"pod, so they must always select the same colour"
            )
        elif live_colours == preview_colours:
            problems.append(
                f"the preview Services select {sorted(preview_colours)}, which is the "
                f"LIVE colour: the deploy's smoke test would pass by testing what is "
                f"already in production, and promote the new colour having checked "
                f"nothing"
            )

    # ---- 5. a pre-install hook may not need a release-managed ServiceAccount
    #
    # Helm applies NO release resource until every pre-install hook has
    # completed, so a hook pod naming a ServiceAccount that the chart declares
    # without a hook annotation can never be admitted:
    #
    #   pods "er-schema-migrate-" is forbidden: error looking up service
    #   account ci/er-api: serviceaccount not found
    #
    # The Job then creates no pod, Helm's hook wait times out, and `--atomic`
    # uninstalls the release. This is invisible on an upgrade — a previous
    # release has already created the ServiceAccount, so only a FIRST install
    # breaks: a new developer namespace, a fresh kind cluster, or a re-install
    # after a rollback uninstalled things. It cost the 2026-11-05 nightly kind
    # run, and no gate would have caught it.
    hook_sas: dict[str, int] = {}
    plain_sas: set[str] = set()
    for doc in platform:
        if doc.get("kind") != "ServiceAccount":
            continue
        ann = (doc["metadata"].get("annotations") or {})
        name = doc["metadata"]["name"]
        if "helm.sh/hook" in ann:
            hook_sas[name] = int(ann.get("helm.sh/hook-weight", 0))
        else:
            plain_sas.add(name)

    pre_hook_pods = 0
    for doc in platform:
        ann = (doc.get("metadata", {}).get("annotations") or {})
        hooks = ann.get("helm.sh/hook", "")
        if not any(h in hooks for h in ("pre-install", "pre-upgrade")):
            continue
        try:
            spec = doc["spec"]["template"]["spec"]
        except (KeyError, TypeError):
            continue  # not a pod-bearing hook (a Secret, a ConfigMap)
        pre_hook_pods += 1
        sa = spec.get("serviceAccountName")
        if sa is None:
            continue
        name = doc["metadata"]["name"]
        weight = int(ann.get("helm.sh/hook-weight", 0))
        if sa in plain_sas:
            problems.append(
                f"the pre-install hook {name!r} names serviceAccountName {sa!r}, "
                f"which this chart declares WITHOUT a helm.sh/hook annotation — so "
                f"it does not exist when the hook runs and the pod is refused. The "
                f"release then fails its first install in any fresh namespace. Either "
                f"drop serviceAccountName (the default SA is right for a hook that "
                f"only reaches Postgres) or give {sa!r} its own hook at a lower weight"
            )
        elif sa in hook_sas and hook_sas[sa] >= weight:
            problems.append(
                f"the pre-install hook {name!r} (weight {weight}) names hook "
                f"ServiceAccount {sa!r} at weight {hook_sas[sa]}. The account must "
                f"sort STRICTLY EARLIER or it may not exist yet"
            )
    if pre_hook_pods == 0:
        problems.append(
            "no pre-install/pre-upgrade hook pod was found in the er-platform "
            "render — the schema and drizzle migrate hooks should both be here, so "
            "this check examined nothing"
        )
    else:
        print(f"  {pre_hook_pods} pre-install hook pod(s); "
              f"hook SAs {sorted(hook_sas)}, release SAs {sorted(plain_sas)}")

    # A developer database must not overwrite the shared queue series. Check
    # both canonical deployments, developer namespaces and crossed env/name pairs.
    for environment, ns, enabled in [
        ("dev", "ci", "true"), ("prod", "app", "true"),
        ("dev", "alice", "false"), ("prod", "alice", "false"),
        ("dev", "app", "false"), ("prod", "ci", "false"),
    ]:
        docs = render([
            "er-platform", REPO_CHART, "--namespace", ns,
            "--set", f"environment={environment}", "--set", f"developer={ns}",
            "--set", "efs.fileSystemId=fs-0", "--set", "efs.accessPointId=fsap-0",
        ])
        publishers = 0
        for doc in docs:
            if doc.get("kind") != "Deployment":
                continue
            for container in doc["spec"]["template"]["spec"]["containers"]:
                settings = {e["name"]: e.get("value") for e in container.get("env", [])}
                if "ER_PG_DSN" not in settings:
                    continue
                publishers += 1
                if settings.get("ER_OPERATIONAL_METRICS") != enabled:
                    problems.append(f"{environment}/{ns}/{container['name']}: operational metric scope must be {enabled}")
        if publishers < 2:
            problems.append(f"{environment}/{ns}: metric scope check missed API or dispatcher")

    # Registration overlap must affect only the edge Service, never recreate
    # a serving pod or erase the canonical live/preview colour selectors.
    for active in ("blue", "green"):
        args = ["er-platform", REPO_CHART, "--namespace", NAMESPACE,
                "--set", "efs.fileSystemId=fs-0", "--set", "efs.accessPointId=fsap-0",
                "--set", "edge.targetGroupArn=arn:test", "--set", f"color.active={active}"]
        normal = render(args)
        overlap = render([*args, "--set", "edge.overlap=true"])
        normal_deploys = {d["metadata"]["name"]: d["spec"] for d in normal if d["kind"] == "Deployment"}
        overlap_deploys = {d["metadata"]["name"]: d["spec"] for d in overlap if d["kind"] == "Deployment"}
        if normal_deploys != overlap_deploys:
            problems.append("edge overlap must not change any Deployment spec")
        for docs, expected_edge in ((normal, {active}), (overlap, {"blue", "green"})):
            ss = {d["metadata"]["name"]: d for d in docs if d["kind"] == "Service"}
            for name in ("er-web", "er-api"):
                if ss[name]["spec"]["selector"].get(COLOR) != active:
                    problems.append(f"{name}: overlap erased the canonical active colour")
            edge = ss["er-web-edge"]["spec"]
            hits = {labels[COLOR] for _, labels in pod_templates(docs)
                    if matches(edge["selector"], labels)}
            if hits != expected_edge or [p["port"] for p in edge["ports"]] != [3000]:
                problems.append(f"edge overlap: expected web-only {expected_edge}, got {hits}")

    if problems:
        print("\ncharts-check FAILED:\n", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print("\ncharts-check passed: every selector resolves, the edge hole is web-only, "
          "and no pre-install hook needs a release-managed ServiceAccount.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
