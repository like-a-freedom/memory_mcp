# Native v9 gate after bounded explain

The Apple Silicon gate ran after the ADR-0084 public schema change.
It replayed four eight-case captures on the same executable. Both
stage repeats agreed. Evidence passed 8/8 in both stages. Decisions
passed 5/8 with the default and 1/8 with explicit shadow, with four gains,
zero losses and zero hard failures. The gate elapsed time was 649.23 seconds.

[Acceptance and replay indexes](claim-rollout-default-acceptance.json)
· [Response-size profile](../../response-size/2026-11-24-bounded-explain-native.json)
· [Native reproduction command](../native-rollout-v9-2026-11-24/report.md)

This repository retains the acceptance report, indexes and per-capture
summaries. Re-run the reproduction command to generate full receipts.
