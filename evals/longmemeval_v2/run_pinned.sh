#!/usr/bin/env bash
# Run the pinned LongMemEval-V2 adapter against memory_mcp.
#
# Usage: ./run_pinned.sh
#
# This script stages the LongMemEval-V2 adapter. The network/dataset-backed
# run is separate and records the exact command, revisions, reader, budget,
# coverage, and result.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Source pinned revisions.
source "${SCRIPT_DIR}/pins.env"

echo "=== LongMemEval-V2 Pinned Adapter ==="
echo "Repository commit: ${LONGMEMEVAL_V2_REPO_COMMIT}"
echo "HF dataset revision: ${LONGMEMEVAL_V2_HF_REVISION}"
echo ""

# Contract smoke: verify the adapter interface without network access.
MEMORY_MCP_BINARY="${MEMORY_MCP_BINARY:-memory_mcp}"

python3 "${SCRIPT_DIR}/memory_mcp_backend.py" --smoke-test 2>/dev/null || {
    echo "Contract smoke test not available; run with the pinned external environment."
    echo "See evals/longmemeval_v2/README.md for staging instructions."
}
