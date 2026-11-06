#!/usr/bin/env bash
# kind.yml's probe needs kind, kubectl and helm. helm comes from
# scripts/ci/install-helm.sh, which owns that pin for all three of its
# consumers; the kind and kubectl pins are local to this probe.
#
# Same reasoning as install-helm.sh for pinned downloads over third-party
# actions, and the trailing version echoes are the same install check.
#
# Run: bash scripts/ci/install-kind-tools.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

KIND_VERSION="${KIND_VERSION:-v0.31.0}"
KUBECTL_VERSION="${KUBECTL_VERSION:-v1.33.4}"

curl -fsSL -o /tmp/kind "https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-linux-amd64"
sudo install /tmp/kind /usr/local/bin/kind

curl -fsSL -o /tmp/kubectl "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl"
sudo install /tmp/kubectl /usr/local/bin/kubectl

bash scripts/ci/install-helm.sh

kind version && kubectl version --client
