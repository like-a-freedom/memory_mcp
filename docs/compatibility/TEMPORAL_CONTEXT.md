# Temporal context and knowledge revisions

Implemented on the working tree based on `1ae1da4a`; see [deterministic quality evidence](../evals/DETERMINISTIC_MEMORY_QUALITY.md) for measured scope. This contract does not claim that all historical retrieval channels are versioned.

## Read contract

`assemble_context` accepts `valid_at` (when the claim applies) and either `known_at` (knowledge committed by that time) or `known_revision` (exact server commit checkpoint). Missing axes use one captured request time. To repeat an effective snapshot across calls, copy `context.snapshot.valid_at` to `valid_at` and `context.snapshot.known_revision` to `known_revision`; omit `as_of` and `known_at`. Repeating a request with omitted axes captures a new snapshot. Revision zero represents the empty revisioned history; negative or future revisions are rejected. Tokens belong to one active namespace/history and are not portable between unrelated stores.

The existing MCP `as_of` parameter maps to **both** axes. It cannot be combined with any new axis. `known_at` and `known_revision` are mutually exclusive. Malformed explicit timestamps are validation errors. Calls without explicit temporal parameters retain current retrieval. The older Rust `AssembleContextRequest.as_of` service interface is distinct; use the temporal capability for the new snapshot contract.

Explicit historical reads support `view_mode=current` only. Responses expose the effective snapshot and `partial` completeness, omitted channels and candidate-scan truncation. Canonical facts are selected at the revision and valid time. Full-question FTS is supplemented with a bounded scan of at most 250 immutable revision-visible facts; direct hits survive that scan. Admission, access, fact type and time-window checks precede the output budget. One recognized contiguous proper name must match the declared structured subject when one exists; a mention in its value cannot substitute for that subject. Numeric components and delimiters remain significant. Separate names and unrecognized/lowercase names retain the lexical policy. Historical FTS membership uses immutable content; ranking excludes global current BM25 statistics and applies deterministic immutable-record ranking. This is conservative lexical grounding, not a general entity resolver.

Aliases, current community summaries, graph expansion, ANN, experience and mutable heat/ranking projections cannot prove historical visibility and remain omitted. Legacy records without revision provenance remain omitted. Empty or reduced results therefore **cannot establish global absence**. Episode fallback preserves source text but has unknown claim role. Current/conflicting/historical roles come from revision-visible relations at the requested valid time; disabled, truncated or unresolved relation coverage remains unknown. Do not promote partial context to complete by suppressing its omissions.

Structured extraction resolves explicit subjects against canonical normalized names only, bypassing fuzzy aliases, including aliases created by earlier ordinary extraction. Unstructured extraction keeps its existing alias policy. This prevents new false merges; it does not repair previously misattributed claims or aliases.

## Write and migration contract

Transaction time and knowledge revisions are server owned. The public MCP/CLI tool rejects caller-supplied `t_ingested`; callers must omit it. The older Rust ingestion request retains the field for compatibility but storage ignores it and assigns server time. Use source/valid time for when an event occurred. This intentionally breaks clients that previously supplied ingestion time; it prevents a later correction from masquerading as earlier knowledge. No privileged timestamp-import API is introduced here.

Existing migration 056 adds the revision clock, durable revision ledger and optional known/closed revision fields for canonical records and claim relations. It does not fabricate a historical ordering for pre-migration rows. Existing current reads can still expose legacy data; revision-safe reads disclose its omission. This execution documents and verifies the previously added migration; it does not change migration files.

Back up and restore the revision clock, ledger and records together. Do not infer historical tokens from content timestamps or compare ordinal tokens across stores. Mixed old/new writers and downgrade after revisioned writes are not a supported historical-consistency protocol; retain a pre-upgrade backup and verify restoration before rollback. No destructive rollback is part of this change.

## Evaluation boundary

Fixed histories replay through public MCP in fresh stores with regex extraction, embeddings disabled, fixed valid-time queries and server-observed revision checkpoints. Raw receipts bind returned episode IDs to seeded event IDs. Binary, configuration, corpus and public schema fingerprints accompany the observations. A schema change requires a separate reviewed compatibility protocol; the current acceptance gate rejects it.

Deterministic acceptance proves the registered finite-cohort decision/evidence conditions and repeat equality. It does not prove latency, general entity resolution, agent task benefit or a universal quality guarantee.
