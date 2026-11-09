#!/usr/bin/env bash
# Preview and schema gates have already passed. The selector-only cutover
# verifies ALB/public health and restores the old colour on failure.
set -euo pipefail
exec python3 "$(dirname "$0")/deploy-cutover.py"
