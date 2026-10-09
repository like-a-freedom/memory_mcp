# B1 temporal relation reads — GREEN

The temporal relation storage contract now passes against the embedded store.
The RED test initially failed to compile because the query type and reader did
not exist; the focused GREEN run exercises both contract cases:

```text
rtk cargo test -p memory_mcp --locked --lib temporal_relation_reads -- --nocapture
exit 0
2 passed; 1782 filtered out
```

The assertions cover relation visibility at transaction-time cutoffs and
policy-aware counterpart hydration, including omission of denied counterpart
identifiers from externally visible results.
