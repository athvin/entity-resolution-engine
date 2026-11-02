#!/usr/bin/env bash
# The kind probe environment (batch-orchestration-design §11 Phase 2):
#
#   scripts/kind_env.sh up      # registry + cluster + image + postgres + chart
#   scripts/kind_env.sh down    # delete the cluster and the registry
#
# `up` is idempotent: an existing registry/cluster is reused, the image is
# rebuilt only when ER_KIND_SKIP_BUILD is unset, and the helm release upgrades
# in place. The data directory (ER_KIND_DATA_DIR, default /tmp/er-kind/srv) is
# the host side of the node's /mnt/er extraMount — the chart's hostPath PV, so
# the probe can write tenant configs onto the "shared filesystem" directly.
#
# The registry wiring is kind's documented local-registry pattern: a registry
# container on the kind docker network, containerd pointed at it through a
# hosts.d entry on every node, and the LocalRegistryHosting ConfigMap for
# tooling that reads it.
set -euo pipefail

CLUSTER=er-kind
NS=er-kind
REG_NAME=er-kind-registry
REG_PORT=5001
IMAGE_TAG=kind
DATA_DIR="${ER_KIND_DATA_DIR:-/tmp/er-kind/srv}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CTX="kind-${CLUSTER}"

log() { echo "kind_env: $*" >&2; }

down() {
    kind delete cluster --name "${CLUSTER}" 2>/dev/null || true
    docker rm -f "${REG_NAME}" 2>/dev/null || true
    log "down: cluster and registry removed (data dir ${DATA_DIR} left in place)"
}

up() {
    # 1. The local registry, on the host's loopback only.
    if [ "$(docker inspect -f '{{.State.Running}}' "${REG_NAME}" 2>/dev/null || true)" != 'true' ]; then
        docker run -d --restart=always -p "127.0.0.1:${REG_PORT}:5000" \
            --network bridge --name "${REG_NAME}" registry:3 >/dev/null
        log "registry ${REG_NAME} started on localhost:${REG_PORT}"
    fi

    # 2. The cluster, with the data mount substituted in.
    mkdir -p "${DATA_DIR}/configs" "${DATA_DIR}/drop" "${DATA_DIR}/data"
    # The pods run as uid 1001 (the image's er user) and hostPath preserves
    # host ownership, so the tree must be writable for them.
    chmod -R a+rwx "${DATA_DIR}"
    if ! kind get clusters 2>/dev/null | grep -qx "${CLUSTER}"; then
        sed "s|__ER_KIND_DATA_DIR__|${DATA_DIR}|" "${ROOT}/infra/k8s/kind/cluster.yaml" \
            | kind create cluster --config=-
    fi

    # 3. containerd → registry, on every node (the documented hosts.d pattern).
    REGISTRY_DIR="/etc/containerd/certs.d/localhost:${REG_PORT}"
    for node in $(kind get nodes --name "${CLUSTER}"); do
        docker exec "${node}" mkdir -p "${REGISTRY_DIR}"
        cat <<EOF | docker exec -i "${node}" cp /dev/stdin "${REGISTRY_DIR}/hosts.toml"
[host."http://${REG_NAME}:5000"]
EOF
    done
    if [ "$(docker inspect -f='{{json .NetworkSettings.Networks.kind}}' "${REG_NAME}")" = 'null' ]; then
        docker network connect kind "${REG_NAME}"
    fi
    kubectl --context "${CTX}" apply -f - <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: local-registry-hosting
  namespace: kube-public
data:
  localRegistryHosting.v1: |
    host: "localhost:${REG_PORT}"
    help: "https://kind.sigs.k8s.io/docs/user/local-registry/"
EOF

    # 4. The image: the one multicall er-api build, host-native arch (the kind
    # node runs whatever the host's docker runs).
    if [ -z "${ER_KIND_SKIP_BUILD:-}" ]; then
        log "building localhost:${REG_PORT}/er-api:${IMAGE_TAG} (set ER_KIND_SKIP_BUILD=1 to reuse)"
        docker build -t "localhost:${REG_PORT}/er-api:${IMAGE_TAG}" -f "${ROOT}/rust/Dockerfile" "${ROOT}/rust"
    fi
    docker push "localhost:${REG_PORT}/er-api:${IMAGE_TAG}" >/dev/null
    log "image pushed"

    # 5. Cluster-scoped objects + the namespace's substrate.
    kubectl --context "${CTX}" apply -f "${ROOT}/infra/k8s/priorityclasses/"
    kubectl --context "${CTX}" create namespace "${NS}" --dry-run=client -o yaml \
        | kubectl --context "${CTX}" apply -f -
    kubectl --context "${CTX}" apply -f "${ROOT}/infra/k8s/kind/postgres.yaml"

    # 6. The secret the chart's ExternalSecrets would have projected. The
    # operator token is minted per cluster and read back on re-up.
    if ! kubectl --context "${CTX}" -n "${NS}" get secret er-erserver-env >/dev/null 2>&1; then
        kubectl --context "${CTX}" -n "${NS}" create secret generic er-erserver-env \
            --from-literal=ERSERVER_DSN="host=er-postgres port=5432 user=er password=er dbname=er" \
            --from-literal=ERSERVER_OPERATOR_TOKEN="$(head -c32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi

    kubectl --context "${CTX}" -n "${NS}" rollout status deploy/er-postgres --timeout=120s

    # 7. The release, under the kind profile.
    helm --kube-context "${CTX}" upgrade --install er-platform \
        "${ROOT}/infra/k8s/charts/er-platform" \
        --namespace "${NS}" \
        -f "${ROOT}/infra/k8s/charts/er-platform/values-kind.yaml" \
        --set "images.registry=localhost:${REG_PORT}" \
        --set "images.tag=${IMAGE_TAG}" \
        --atomic --wait --timeout 5m
    log "up: er-platform installed in ${NS} (kubectl --context ${CTX} -n ${NS} get pods)"
}

case "${1:-}" in
    up) up ;;
    down) down ;;
    *) echo "usage: $0 up|down" >&2; exit 2 ;;
esac
