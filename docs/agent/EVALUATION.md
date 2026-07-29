# Evaluation

The `eval-harness` crate (`memory-eval` binary) provides profile-driven evaluation. It is never linked into the production binary.

## Profiles

```bash
# PR profile (deterministic regression, target 10 min)
make eval-pr

# Release profile (full retrieval + lifecycle, target 20 min)
make eval-release

# Nightly profile (full end-to-end + diagnostics)
make eval-nightly
```

## Corpus Preparation (one-time, requires network)

```bash
cargo run -p eval-harness --bin memory-eval -- prepare-corpus \
  --manifest evals/corpora/longmemeval.json \
  --output-root data/corpora
```

## Performance Benchmarks

Criterion benchmarks, separate from `cargo test`:

```bash
cargo bench -p eval-harness --bench pipeline -- --noplot
cargo bench -p eval-harness --bench ner_cpu -- --noplot
cargo bench -p eval-harness --bench contention -- --noplot
```
