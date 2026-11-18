#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fixture_target="$repo_root/target/surrealdb3-fixture"
cargo build \
  --manifest-path crates/memory-mcp/tests/fixtures/surrealdb_3x_fixture/Cargo.toml \
  --target-dir "$fixture_target" \
  --locked

if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
  fixture_binary="$fixture_target/$CARGO_BUILD_TARGET/debug/sdb3-fixture"
else
  fixture_binary="$fixture_target/debug/sdb3-fixture"
fi
if [[ ! -x "$fixture_binary" ]]; then
  echo "SurrealDB 3.x fixture binary not found: $fixture_binary" >&2
  exit 1
fi

export SURREALDB3_FIXTURE_BIN="$fixture_binary"
cargo test -p memory_mcp \
  --features streamable-http \
  --test surrealdb4_upgrade \
  --locked \
  -- --ignored --test-threads=1
