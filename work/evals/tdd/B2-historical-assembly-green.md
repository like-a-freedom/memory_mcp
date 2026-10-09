# B2 historical assembly projection — GREEN

The real embedded-store historical assembly regression now passes:

```text
rtk cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_reconciliation exposes_supersession_on_the_predecessor_item -- --exact --test-threads=1
exit 0
1 passed; 5 filtered out
```

The selected predecessor item carries the persisted supersession relation as
of the requested historical cutoff. The relation read remains constrained to
the selected pack and uses the request's access policy. Retrieval does not
rescue an otherwise unadmitted counterpart solely because it is related.
