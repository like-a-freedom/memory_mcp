# A3 structured evidence RED

The previous admission implementation had no typed community-summary or repeated-topic proof. The two new unit expectations compiled and failed as intended; the real-store test then exposed that a weak direct hit was removed from the community path before its summary evidence could be fused.

- Admission RED: `rtk cargo test -p memory_mcp --locked --lib memory::retrieval::admission::tests` — 2 behavioral failures, 23 passed. Log SHA-256: `cf527275e618474c2fa4ea4b6dc0468a29b6234ea4b1843bddf6c80610f6a94a`.
- Embedded-store RED: `rtk cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality community_summary_admits_a_persisted_member_fact_with_only_one_fact_term` — returned zero items where the persisted member fact was expected. Log SHA-256: `3c2e969c17085e1dc7c75f30e2a4aa08edc4893bdb5aa1c419712d26d9addbe0`.

The RED-time source blob was not captured. The test logs are preserved and hashed; no source hash is invented.
