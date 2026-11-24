# Native v9 default rollout regression gate

The locked native Apple Silicon run used Rust 1.100.0
(`0ef1bc386059596dada8fb65b50425aab2ea637a`, host
`aarch64-apple-darwin`) and a Mach-O arm64 binary. It exercised the
B4 product source with the `streamable-http` feature.

The gate executed one real-product test with four captures and eight
cases per capture. The repeated indexes matched within each stage.
The evidence count was 8/8 in both stages. Decision success was 5/8
with the product default and 1/8 in explicit shadow mode. There were
four gains, zero losses and zero hard failures. The comparison uses
two stages of one executable. The cross-build B4 results are in the
[identity report](../historical-identity-2026-11-24/report.md).
[Acceptance report](claim-rollout-default-acceptance.json)

```sh
cargo build -p memory_mcp --bin memory_mcp --features streamable-http --locked
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_QUALITY_GATE_OUTPUT="$(mktemp -d)/native-quality" \
cargo test -p eval-harness --locked --test deterministic_quality_gate -- --ignored
```

The test elapsed time was 729.71 seconds. That is end-to-end
fixture and harness runtime.
