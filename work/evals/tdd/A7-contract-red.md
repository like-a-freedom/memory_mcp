# A7 wire contract RED evidence

Base revision: `355dc92206e341d02a61a6066cd7ec5e49f21ce0`.

The behavior-first integration target invoked the real protocol-agnostic tool
against an isolated embedded SurrealDB store and inspected the serialized
`ToolResponse` JSON.

- `rtk cargo test -p memory_mcp --locked --test assemble_context_wire_contract -- --test-threads=1`
  exited 101. Log SHA-256:
  `860520fa47150c4ef917131b1ec0a9569a40a008ceaa9e20d63a579464ea8aa2`.
  The empty response returned the nonempty-result `explain` guidance, and a
  selected verbose item had no `retrieval_evidence` field.
- The same command after adding the rationale score assertion exited 101.
  Log SHA-256:
  `79829e03d4f93b0cb9beb570fd4f4ba962537d6c1e384dea7d0ec9d0c6f5ebfe`.
  The selected item printed `relevance=0.90` while its serialized final
  `relevance` was `0.32`; the empty guidance mismatch remained.

These were assertion failures after successful compilation and real-store
setup. The exact pre-RED source blob was not captured, so no source hash is
claimed.
