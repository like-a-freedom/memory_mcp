#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
FIXTURES="$SCRIPT_DIR/measure_ttv_fixtures"

expect_rejection() {
  local kind="$1"
  local path="$2"
  if "$SCRIPT_DIR/measure_ttv.sh" --validate-fixture "$kind" "$path"; then
    printf 'fixture unexpectedly accepted: %s %s\n' "$kind" "$path" >&2
    exit 1
  fi
}

expect_rejection json "$FIXTURES/invalid.json"
expect_rejection json "$FIXTURES/missing-result.json"
expect_rejection facts "$FIXTURES/empty-facts.json"
expect_rejection context "$FIXTURES/fallback-only.json"
printf '%s\n' 'rejected 4 invalid TTV fixtures as expected'
