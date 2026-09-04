#!/usr/bin/env bash
# scripts/http_release_evidence.sh
#
# Run the HTTP release evidence suite. Two modes:
#
#   local    Run only the automated gates that can be exercised on a
#            developer laptop or in CI. External gates that require
#            a specific environment are recorded as `not_executed`
#            and the script exits 0.
#
#   release  Same as `local`, but the script also fails (exit 1)
#            if any external gate row is `not_executed`. Used by
#            release engineering to verify the full matrix is
#            covered before tagging.
#
# Override the default mode by setting MEMORY_MCP_HTTP_RELEASE=1.
#
# All gate output is captured under `target/http-release-evidence/<ts>/`:
#
#   manifest.env   git rev, rustc/cargo versions, hostname, timestamp
#   gates.tsv        one row per gate (gate, command, commit, timestamp,
#                     environment, result, evidence_path)
#   <gate>.log       per-gate command output (or "not_executed")
#
# Run:
#   scripts/http_release_evidence.sh local
#   MEMORY_MCP_HTTP_RELEASE=1 scripts/http_release_evidence.sh release

set -euo pipefail

MODE="${1:-${MEMORY_MCP_HTTP_RELEASE:+release}}"
MODE="${MODE:-local}"

# Resolve workspace root. The script may run from anywhere; we
# want it to record relative paths from the workspace root.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

# Evidence directory. The timestamp is taken at the start of the
# run so multiple invocations don't collide.
TS="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE_DIR="target/http-release-evidence/${TS}"
mkdir -p "${EVIDENCE_DIR}"

# Environment and toolchain manifest. Capture once at the top so the
# rest of the run can reference these values deterministically.
COMMIT="$(git rev-parse HEAD)"
RUSTC_VERSION="$(rustc --version)"
CARGO_VERSION="$(cargo --version)"
HOSTNAME_VAL="$(hostname)"

cat > "${EVIDENCE_DIR}/manifest.env" <<EOF
timestamp=${TS}
commit=${COMMIT}
hostname=${HOSTNAME_VAL}
rustc=${RUSTC_VERSION}
cargo=${CARGO_VERSION}
mode=${MODE}
EOF

# gates.tsv column order is fixed: gate, command, commit,
# timestamp, environment, result, evidence_path. Use tabs so
# downstream tooling can parse it portably.
GATES_TSV="${EVIDENCE_DIR}/gates.tsv"
printf 'gate\tcommand\tcommit\ttimestamp\tenvironment\tresult\tevidenc
e_path\n' > "${GATES_TSV}"

# Run a single gate. The function captures exit code, writes the
# per-gate log, and appends a row to gates.tsv. The caller decides
# whether to invoke the command or to record a `not_executed`
# row. `result` is `pass`, `fail`, or `not_executed`.
#
#   $1 gate name
#   $2 command (the literal command line that produced the evidence)
#   $3 environment (the env var that gated the run, or "local")
#   $4 result (pass/fail/not_executed)
#   $5 evidence_path (relative to the evidence dir)
run_gate() {
    local gate="$1"
    local cmd="$2"
    local env_gate="$3"
    local result="$4"
    local evidence_path="$5"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "${gate}" "${cmd}" "${COMMIT}" "${TS}" "${env_gate}" "${result}" "${evidence_path}" \
        >> "${GATES_TSV}"
}

run_local() {
    local gate="$1"
    local cmd="$2"
    local log="${EVIDENCE_DIR}/${gate}.log"
    set +e
    bash -c "${cmd}" > "${log}" 2>&1
    local code=$?
    set -e
    if [[ ${code} -eq 0 ]]; then
        run_gate "${gate}" "${cmd}" "local" "pass" "${gate}.log"
    else
        run_gate "${gate}" "${cmd}" "local" "fail" "${gate}.log"
        FAILED_LOCAL=1
    fi
}

run_external_blocked() {
    local gate="$1"
    local cmd="$2"
    local env_gate="$3"
    local log="${EVIDENCE_DIR}/${gate}.log"
    cat > "${log}" <<EOF
not_executed
External gate '${gate}' requires ${env_gate}=1.
Run with the env gate enabled to produce evidence.
Command that would be executed: ${cmd}
EOF
    run_gate "${gate}" "${cmd}" "${env_gate}" "not_executed" "${gate}.log"
}

FAILED_LOCAL=0

# ---------------------------------------------------------------------------
# Local automated gates. Always run in `local` mode; in `release` mode
# the local gates still run so the manifest captures current health.
# ---------------------------------------------------------------------------

run_local "fmt" "cargo fmt --all --check"

run_local "clippy" \
    "cargo clippy -p memory_mcp --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,prometheus,test-fixtures --locked -- -D warnings"

# Test suites. Every HTTP e2e binary must run with --test-threads=1
# because they share an embedded SurrealDB lock file path and
# fixture port range. A multi-thread run races fixtures.
FEATURE_FLAGS="streamable-http,mcp-apps,control-plane,test-fixtures"

# When the proxy env gate is set, forward it to the
# http_proxy_streaming test so the proxy gate test
# (http_proxy_streaming_proxy_gate, marked `#[ignore]`) inside
# the suite has its required env var when explicitly invoked.
PROXY_ENV_FORWARD=""
if [[ -n "${MEMORY_MCP_HTTP_PROXY_BIN:-}" ]]; then
    PROXY_ENV_FORWARD="MEMORY_MCP_TEST_PROXY_BIN='${MEMORY_MCP_HTTP_PROXY_BIN}'"
fi

for suite in http_proto_conformance http_isolation http_proxy_streaming http_control_plane http_crash_recovery http_durable_tasks http_subscription_replica http_load_concurrency http_registry_storage; do
    cmd="${PROXY_ENV_FORWARD} cargo test -p memory_mcp --features ${FEATURE_FLAGS} --test ${suite} -- --test-threads=1"
    run_local "${suite}" "${cmd}"
done

# ---------------------------------------------------------------------------
# External gates. Each is gated by an env var. When the env var is
# unset we record `not_executed` and continue; in release mode the
# rows surface as blocking failures.
# ---------------------------------------------------------------------------

if [[ -n "${MEMORY_MCP_HTTP_PROXY_BIN:-}" ]]; then
    # Run the (ignored) proxy gate test with the env var forwarded.
    # The default `cargo test --test http_proxy_streaming` invocation
    # already ran the non-gated tests; this adds the proxy gate.
    cmd="MEMORY_MCP_TEST_PROXY_BIN='${MEMORY_MCP_HTTP_PROXY_BIN}' cargo test -p memory_mcp --features ${FEATURE_FLAGS} --test http_proxy_streaming http_proxy_streaming_proxy_gate -- --test-threads=1 --ignored"
    run_local "http_proxy_streaming_proxy_gate" "${cmd}"
else
    run_external_blocked "http_proxy_streaming_proxy_gate" \
        "MEMORY_MCP_TEST_PROXY_BIN=<proxy> cargo test -p memory_mcp --features ${FEATURE_FLAGS} --test http_proxy_streaming http_proxy_streaming_proxy_gate -- --test-threads=1 --ignored" \
        "MEMORY_MCP_HTTP_PROXY_BIN"
fi

if [[ -n "${MEMORY_MCP_HTTP_500_TENANT:-}" ]]; then
    run_local "http_load_concurrency_500" \
        "MEMORY_MCP_HTTP_500_TENANT=1 cargo test -p memory_mcp --features ${FEATURE_FLAGS} --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1 --ignored"
else
    run_external_blocked "http_load_concurrency_500" \
        "MEMORY_MCP_HTTP_500_TENANT=1 cargo test -p memory_mcp --features ${FEATURE_FLAGS} --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1 --ignored" \
        "MEMORY_MCP_HTTP_500_TENANT"
fi

if [[ -n "${MEMORY_MCP_HTTP_INTEROP_CLIENTS_DIR:-}" ]]; then
    # Placeholder for an interop matrix runner that drives each
    # client against the deployed server. The matrix script is
    # out of scope for this evidence script.
    run_external_blocked "http_interop_matrix_clients" \
        "${MEMORY_MCP_HTTP_INTEROP_CLIENTS_DIR}/run.sh --manifest docs/operations/HTTP_INTEROP_MATRIX.md" \
        "MEMORY_MCP_HTTP_INTEROP_CLIENTS_DIR"
else
    run_external_blocked "http_interop_matrix_clients" \
        "<interop-clients-dir>/run.sh --manifest docs/operations/HTTP_INTEROP_MATRIX.md" \
        "MEMORY_MCP_HTTP_INTEROP_CLIENTS_DIR"
fi

if [[ -n "${MEMORY_MCP_HTTP_RESTORE_DRILL_DB:-}" ]]; then
    run_external_blocked "restore_drill" \
        "scripts/restore_drill.sh ${MEMORY_MCP_HTTP_RESTORE_DRILL_DB}" \
        "MEMORY_MCP_HTTP_RESTORE_DRILL_DB"
else
    run_external_blocked "restore_drill" \
        "scripts/restore_drill.sh <target-db>" \
        "MEMORY_MCP_HTTP_RESTORE_DRILL_DB"
fi

if [[ -n "${MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET:-}" ]]; then
    run_external_blocked "credential_rotation" \
        "scripts/credential_rotation.sh ${MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET}" \
        "MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET"
else
    run_external_blocked "credential_rotation" \
        "scripts/credential_rotation.sh <target-deployment>" \
        "MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET"
fi

# ---------------------------------------------------------------------------
# Decision. The script returns nonzero if any local gate failed or
# any external gate is `not_executed` while running in release mode.
# ---------------------------------------------------------------------------

if [[ "${MODE}" == "release" ]]; then
    # Surface any `not_executed` row as a release-blocker.
    # BSD grep on macOS does not support -P; use awk for portability.
    if awk -F'\t' '$6 == "not_executed"' "${GATES_TSV}" | grep -q .; then
        echo
        echo "Release gate FAILED: external gate rows are not_executed." >&2
        echo "Re-run with the env gate set, or update docs/operations/HTTP_INTEROP_MATRIX.md" >&2
        echo "and docs/operations/HTTP_RELEASE_GATE.md with the evidence." >&2
        exit 1
    fi
fi

if [[ "${FAILED_LOCAL}" -ne 0 ]]; then
    echo
    echo "Local gate(s) FAILED. See ${GATES_TSV} for which gate failed and ${EVIDENCE_DIR}/<gate>.log for output." >&2
    exit 1
fi

echo "Release evidence captured at ${EVIDENCE_DIR}"
echo "Manifest: ${EVIDENCE_DIR}/manifest.env"
echo "Gate matrix: ${GATES_TSV}"