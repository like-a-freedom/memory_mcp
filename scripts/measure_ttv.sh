#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/measure_ttv.sh --binary PATH --persona release-binary [--repeat N]
  scripts/measure_ttv.sh --cargo-install --source PATH --persona rust-user [--repeat N]
  scripts/measure_ttv.sh --binary PATH --persona host-config-user [--repeat N]
  scripts/measure_ttv.sh --validate-fixture KIND PATH

KIND is one of: json, facts, context.
USAGE
}

absolute_path() {
  python3 - "$1" <<'PY'
import os
import sys
print(os.path.abspath(sys.argv[1]))
PY
}

validate_response() {
  python3 - "$1" "$2" <<'PY'
import json
import sys

kind, path = sys.argv[1:]
try:
    with open(path, encoding="utf-8") as handle:
        payload = json.load(handle)
except (OSError, json.JSONDecodeError) as error:
    print(f"invalid {kind} JSON: {error}", file=sys.stderr)
    raise SystemExit(1)

if not isinstance(payload, dict) or "result" not in payload:
    print("response is missing top-level result", file=sys.stderr)
    raise SystemExit(1)

result = payload["result"]
if kind == "json":
    raise SystemExit(0)
if kind == "facts":
    if not isinstance(result, dict) or not isinstance(result.get("facts"), list) or not result["facts"]:
        print("response contains no extracted facts", file=sys.stderr)
        raise SystemExit(1)
    raise SystemExit(0)
if kind == "context":
    if not isinstance(result, list):
        print("context result is not an array", file=sys.stderr)
        raise SystemExit(1)
    for item in result:
        if not isinstance(item, dict):
            continue
        fact_id = item.get("fact_id")
        text = f'{item.get("content", "")} {item.get("quote", "")}'.lower()
        if (
            isinstance(fact_id, str)
            and not fact_id.startswith("episode_fallback:")
            and "ada" in text
            and "memory mcp" in text
        ):
            raise SystemExit(0)
    print("context contains no real fact recall", file=sys.stderr)
    raise SystemExit(1)

print(f"unsupported validator kind: {kind}", file=sys.stderr)
raise SystemExit(2)
PY
}

validate_init_output() {
  python3 - "$1" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
if payload.get("target") != "vscode":
    raise SystemExit("init result target is not vscode")
if payload.get("mutates_files") is not False:
    raise SystemExit("init result is not non-mutating")
if not isinstance(payload.get("snippet"), str):
    raise SystemExit("init result has no snippet string")
json.loads(payload["snippet"])
PY
}

validate_init_and_write() {
  python3 - "$1" "$2" <<'PY'
import json
import os
import sys

input_path, output_path = sys.argv[1:]
with open(input_path, encoding="utf-8") as handle:
    payload = json.load(handle)
if payload.get("target") != "vscode":
    raise SystemExit("init result target is not vscode")
if payload.get("mutates_files") is not False:
    raise SystemExit("init result is not non-mutating")
snippet = payload.get("snippet")
if not isinstance(snippet, str):
    raise SystemExit("init result has no snippet string")
parsed = json.loads(snippet)
os.makedirs(os.path.dirname(output_path), exist_ok=True)
with open(output_path, "w", encoding="utf-8") as handle:
    json.dump(parsed, handle, indent=2)
    handle.write("\n")
PY
}

monotonic_ns() {
  python3 -c 'import time; print(time.monotonic_ns())'
}

seconds_between() {
  python3 - "$1" "$2" <<'PY'
import sys
print(f"{(int(sys.argv[2]) - int(sys.argv[1])) / 1_000_000_000:.6f}")
PY
}

run_timed_to_file() {
  local output="$1"
  shift
  local started finished
  started=$(monotonic_ns)
  "$@" >"$output"
  finished=$(monotonic_ns)
  LAST_ELAPSED=$(seconds_between "$started" "$finished")
}

run_timed_install() {
  local log_path="$1"
  shift
  local started finished
  started=$(monotonic_ns)
  if ! cargo install "$@" >"$log_path" 2>&1; then
    cat "$log_path" >&2
    return 1
  fi
  finished=$(monotonic_ns)
  LAST_ELAPSED=$(seconds_between "$started" "$finished")
}

PERSONA=""
BINARY_INPUT=""
SOURCE_INPUT=""
CARGO_INSTALL=false
REPEAT=1
VALIDATE_KIND=""
VALIDATE_PATH=""

while (($# > 0)); do
  case "$1" in
    --binary)
      (($# >= 2)) || { usage; exit 2; }
      BINARY_INPUT="$2"
      shift 2
      ;;
    --cargo-install)
      CARGO_INSTALL=true
      shift
      ;;
    --source)
      (($# >= 2)) || { usage; exit 2; }
      SOURCE_INPUT="$2"
      shift 2
      ;;
    --persona)
      (($# >= 2)) || { usage; exit 2; }
      PERSONA="$2"
      shift 2
      ;;
    --repeat)
      (($# >= 2)) || { usage; exit 2; }
      REPEAT="$2"
      shift 2
      ;;
    --validate-fixture)
      (($# >= 3)) || { usage; exit 2; }
      VALIDATE_KIND="$2"
      VALIDATE_PATH="$3"
      shift 3
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -n "$VALIDATE_KIND" ]]; then
  [[ -z "$PERSONA" && -z "$BINARY_INPUT" && "$CARGO_INSTALL" == false ]] || {
    printf '%s\n' '--validate-fixture cannot be combined with a measurement persona' >&2
    exit 2
  }
  validate_response "$VALIDATE_KIND" "$VALIDATE_PATH"
  exit $?
fi

[[ "$REPEAT" =~ ^[1-9][0-9]*$ ]] || {
  printf '%s\n' '--repeat must be a positive integer' >&2
  exit 2
}
case "$PERSONA" in
  release-binary|host-config-user)
    [[ "$CARGO_INSTALL" == false && -n "$BINARY_INPUT" ]] || {
      printf '%s\n' "persona $PERSONA requires --binary and does not use --cargo-install" >&2
      exit 2
    }
    ;;
  rust-user)
    [[ "$CARGO_INSTALL" == true ]] || {
      printf '%s\n' 'persona rust-user requires --cargo-install' >&2
      exit 2
    }
    SOURCE_INPUT="${SOURCE_INPUT:-.}"
    ;;
  *)
    printf '%s\n' '--persona must be release-binary, rust-user, or host-config-user' >&2
    exit 2
    ;;
esac

if [[ -n "$BINARY_INPUT" ]]; then
  BINARY_INPUT=$(absolute_path "$BINARY_INPUT")
  [[ -x "$BINARY_INPUT" ]] || {
    printf 'binary is not executable: %s\n' "$BINARY_INPUT" >&2
    exit 1
  }
fi
if [[ -n "$SOURCE_INPUT" ]]; then
  SOURCE_INPUT=$(absolute_path "$SOURCE_INPUT")
  [[ -d "$SOURCE_INPUT" ]] || {
    printf 'source directory does not exist: %s\n' "$SOURCE_INPUT" >&2
    exit 1
  }
fi

ORIGINAL_PATH="${PATH:-}"
TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "$TEMP_ROOT"' EXIT
SAMPLES_FILE="$TEMP_ROOT/samples.jsonl"
: >"$SAMPLES_FILE"

for run_number in $(seq 1 "$REPEAT"); do
  TEMP="$TEMP_ROOT/run-$run_number"
  HOME_DIR="$TEMP/home"
  XDG_DATA_HOME="$TEMP/xdg"
  CARGO_HOME_DIR="$TEMP/cargo"
  WORKDIR="$TEMP/work"
  mkdir -p "$HOME_DIR" "$XDG_DATA_HOME" "$CARGO_HOME_DIR" "$WORKDIR"

  for key in \
    SURREALDB_URL SURREALDB_EMBEDDED SURREALDB_DB_NAME SURREALDB_NAMESPACES \
    SURREALDB_USERNAME SURREALDB_PASSWORD SURREALDB_DATA_DIR \
    SURREALDB_EMBEDDING_DIMENSION RUST_LOG QUERY_LOGGING_ENABLED \
    QUERY_LOG_RETENTION_DAYS LIFECYCLE_ENABLED LIFECYCLE_DECAY_INTERVAL_SECS \
    LIFECYCLE_ARCHIVAL_INTERVAL_SECS LIFECYCLE_DECAY_THRESHOLD \
    LIFECYCLE_ARCHIVAL_AGE_DAYS LIFECYCLE_DECAY_HALF_LIFE_DAYS \
    EMBEDDINGS_ENABLED EMBEDDINGS_PROVIDER EMBEDDINGS_TIMEOUT_SECS \
    EMBEDDINGS_MAX_TOKENS EMBEDDINGS_SIMILARITY_THRESHOLD EMBEDDINGS_MODEL_DIR \
    EMBEDDINGS_MODEL EMBEDDINGS_BASE_URL EMBEDDINGS_API_KEY NER_PROVIDER \
    NER_MODEL NER_LABELS NER_THRESHOLD NER_BATCH_SIZE NER_MAX_BATCH_TOKENS \
    NER_MAX_CONCURRENCY NER_DEVICE NER_MODEL_DIR GLINER_IDLE_UNLOAD_SECS \
    MEMORY_CLAIM_ROLLOUT_STAGE MEMORY_CLAIM_CANDIDATE_PAGE_SIZE \
    MEMORY_CLAIM_INLINE_CANDIDATE_LIMIT MEMORY_CLAIM_INLINE_BUDGET_MS \
    ENTITY_FUZZY_THRESHOLD; do
    unset "$key"
  done
  export HOME="$HOME_DIR"
  export XDG_DATA_HOME
  export CARGO_HOME="$CARGO_HOME_DIR"
  export PATH="$ORIGINAL_PATH"
  cd "$WORKDIR"
  mkdir -p "$XDG_DATA_HOME/memory_mcp"

  total_started=$(monotonic_ns)
  install_seconds="0.000000"
  if [[ "$PERSONA" == "rust-user" ]]; then
    run_timed_install "$TEMP/install.log" \
      --path "$SOURCE_INPUT/crates/memory-mcp" \
      --locked \
      --root "$TEMP/install"
    install_seconds="$LAST_ELAPSED"
    BIN="$TEMP/install/bin/memory_mcp"
    [[ -x "$BIN" ]] || { printf '%s\n' 'cargo install did not produce memory_mcp' >&2; exit 1; }
  else
    BIN="$BINARY_INPUT"
  fi

  INIT_JSON="$TEMP/init.json"
  INGEST_JSON="$TEMP/ingest.json"
  EXTRACT_JSON="$TEMP/extract.json"
  CONTEXT_JSON="$TEMP/context.json"
  run_timed_to_file "$INIT_JSON" "$BIN" init --target vscode
  init_seconds="$LAST_ELAPSED"
  validate_init_output "$INIT_JSON"
  if [[ "$PERSONA" == "host-config-user" ]]; then
    validate_init_and_write "$INIT_JSON" "$WORKDIR/.vscode/mcp.json"
  fi

  run_timed_to_file "$INGEST_JSON" "$BIN" ingest \
    --source-type requirement \
    --source-id ttv-fixture \
    --content "Ada owns the memory MCP project; platform integrations are ready, stakeholder approvals are pending, and response workflow is scoped." \
    --t-ref 2026-08-04T00:00:00Z
  episode_write_seconds="$LAST_ELAPSED"
  validate_response json "$INGEST_JSON"
  episode_id=$(python3 - "$INGEST_JSON" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    episode_id = json.load(handle)["result"]
if not isinstance(episode_id, str) or not episode_id:
    raise SystemExit("ingest result is not an episode ID")
print(episode_id)
PY
)

  run_timed_to_file "$EXTRACT_JSON" "$BIN" extract --episode-id "$episode_id"
  extraction_seconds="$LAST_ELAPSED"
  validate_response facts "$EXTRACT_JSON"

  run_timed_to_file "$CONTEXT_JSON" "$BIN" assemble-context \
    --query "Who owns the memory MCP project?" \
    --scope org
  fact_recall_seconds="$LAST_ELAPSED"
  validate_response context "$CONTEXT_JSON"
  total_finished=$(monotonic_ns)
  total_seconds=$(seconds_between "$total_started" "$total_finished")

  printf '{"install_seconds":%s,"init_seconds":%s,"episode_write_seconds":%s,"extraction_seconds":%s,"fact_recall_seconds":%s,"total_seconds":%s,"success":true}\n' \
    "$install_seconds" "$init_seconds" "$episode_write_seconds" "$extraction_seconds" "$fact_recall_seconds" "$total_seconds" >>"$SAMPLES_FILE"
done

python3 - "$PERSONA" "$SAMPLES_FILE" <<'PY'
import json
import math
import statistics
import sys

persona, samples_path = sys.argv[1:]
with open(samples_path, encoding="utf-8") as handle:
    samples = [json.loads(line) for line in handle if line.strip()]

fields = [
    "install_seconds",
    "init_seconds",
    "episode_write_seconds",
    "extraction_seconds",
    "fact_recall_seconds",
    "total_seconds",
]

def aggregate(field, percentile):
    values = sorted(sample[field] for sample in samples)
    if percentile == "median":
        return round(statistics.median(values), 6)
    index = max(0, math.ceil(0.90 * len(values)) - 1)
    return round(values[index], 6)

print(json.dumps({
    "persona": persona,
    "runs": len(samples),
    "samples": samples,
    "median_seconds": {field: aggregate(field, "median") for field in fields},
    "p90_seconds": {field: aggregate(field, "p90") for field in fields},
    "success": all(sample.get("success") is True for sample in samples),
}, indent=2))
PY
