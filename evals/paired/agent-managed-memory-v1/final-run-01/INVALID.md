# Invalid capture: unsupported-source count constraint

The v3 full capture wrote 15 of 16 arm receipts, then its stage validator rejected a valid independent-probe observation. The scorer correctly represented one unmapped episode ID as `unsupported_source_count=1` and no known event IDs; the validator incorrectly required unsupported count not to exceed known-source count. The regression `paired_run_retains_unmapped_independent_probe_sources_as_negative_evidence` failed before the fix and passes afterward. The capture is retained for diagnosis but excluded from summaries.

The v4 run set uses the rebuilt evaluator SHA recorded in the v4 plan and starts fresh.
