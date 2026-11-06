#!/usr/bin/env bash
# infra.yml's chart tier needs helm to render and kubeconform to schema-check
# what it rendered. helm comes from scripts/ci/install-helm.sh, which owns that
# pin for all three of its consumers; only the kubeconform pin is local.
#
# Same reasoning as install-helm.sh for the pinned tarball over a third-party
# action, and the trailing `kubeconform -v` is the same install check: a gate
# that cannot run must fail at a named step rather than become a missing
# binary inside a later script's output.
#
# Run: bash scripts/ci/install-chart-tools.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

KUBECONFORM_VERSION="${KUBECONFORM_VERSION:-v0.7.0}"

bash scripts/ci/install-helm.sh

curl -fsSL "https://github.com/yannh/kubeconform/releases/download/${KUBECONFORM_VERSION}/kubeconform-linux-amd64.tar.gz" |
	sudo tar xz -C /usr/local/bin kubeconform
kubeconform -v
