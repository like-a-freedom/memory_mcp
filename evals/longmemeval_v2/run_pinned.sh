#!/usr/bin/env bash
# Run the pinned LongMemEval-V2 adapter against memory_mcp.
#
# Usage: ./run_pinned.sh --smoke-only
#        ./run_pinned.sh --integration-smoke BINARY
#
# This script stages the LongMemEval-V2 adapter. The network/dataset-backed
# run is separate and records the exact command, revisions, reader, budget,
# coverage, and result.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}/../.."

# Source pinned revisions.
source "${SCRIPT_DIR}/pins.env"

test -f "${SCRIPT_DIR}/profile.json" || {
    echo "missing LongMemEval-V2 profile.json" >&2
    exit 2
}

echo "=== LongMemEval-V2 Pinned Adapter ==="
echo "Repository commit: ${LONGMEMEVAL_V2_REPO_COMMIT}"
echo "HF dataset revision: ${LONGMEMEVAL_V2_HF_REVISION}"
echo "Evaluation profile: ${SCRIPT_DIR}/profile.json"
echo ""

case "${1:-}" in
    --smoke-only)
        python3 "${SCRIPT_DIR}/memory_mcp_backend.py" --smoke-test
        ;;
    --integration-smoke)
        python3 "${SCRIPT_DIR}/memory_mcp_backend.py" --smoke-test
        if [[ -z "${2:-}" ]]; then
            echo "usage error: --integration-smoke requires a binary path" >&2
            exit 2
        fi
        MEMORY_MCP_BINARY="$2" python3 - <<'PY'
import os
import tempfile
from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend

with tempfile.TemporaryDirectory(prefix="memory-mcp-lme-smoke-") as db_path:
    backend = MemoryMcpBackend({"binary": os.environ["MEMORY_MCP_BINARY"], "db_path": db_path})
    backend.insert({"role": "user", "content": "I prefer tea"})
    items = backend.query("What do I prefer?")
    assert items and all(item["type"] == "text" for item in items)
    assert any("tea" in item["value"].lower() for item in items), items
print("LongMemEval-V2 integration smoke: PASS")
PY
        ;;
    *)
        echo "no benchmark executed: pass --smoke-only or --integration-smoke BINARY" >&2
        echo "the official upstream launcher and pinned dataset are required for a leaderboard result" >&2
        exit 2
        ;;
esac
