#!/usr/bin/env bash
# Install helm from its pinned release tarball. THE PIN LIVES HERE.
#
# Three workflows installed helm with an identical six-line copy of this —
# infra.yml's chart tier, deploy.yml's cutover and kind.yml's probe — each
# carrying its own `HELM_VERSION: v3.19.0`. Three copies of a version number
# is three places to bump and two places to forget, and the forgetting is
# silent: the chart tier would lint against one helm while the deploy renders
# with another. One copy, overridable per call site by exporting HELM_VERSION
# when a workflow genuinely needs a different one.
#
# A VERSION-PINNED TARBALL RATHER THAN A THIRD-PARTY ACTION, deliberately:
# every `uses:` in this repo is SHA-pinned (rust.yml's `workflow pins (S9)`
# job enforces it), and a pinned download needs no SHA to stay current. The
# trailing `helm version --short` is the check that the install worked, rather
# than leaving a missing binary for a later step to trip over — which is how
# this reads as a chart error three steps downstream.
#
# The comments this replaced each cited "images.yml's helm install" as the
# precedent for that choice; images.yml no longer installs helm, so the
# argument is stated here instead of pointed at.
#
# Run: bash scripts/ci/install-helm.sh
set -euo pipefail

HELM_VERSION="${HELM_VERSION:-v3.19.0}"

curl -fsSL "https://get.helm.sh/helm-${HELM_VERSION}-linux-amd64.tar.gz" |
	sudo tar xz --strip-components=1 -C /usr/local/bin linux-amd64/helm
helm version --short
