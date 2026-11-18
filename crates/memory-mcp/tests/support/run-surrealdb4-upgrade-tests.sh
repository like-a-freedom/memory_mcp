#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fixture_target="$repo_root/target/surrealdb3-fixture"
cargo build \
  --manifest-path crates/memory-mcp/tests/fixtures/surrealdb_3x_fixture/Cargo.toml \
  --target-dir "$fixture_target" \
  --locked

export SURREALDB3_FIXTURE_BIN="$fixture_target/debug/sdb3-fixture"
cargo test -p memory_mcp \
  --features streamable-http \
  --test surrealdb4_upgrade \
  --locked \
  -- --test-threads=1
