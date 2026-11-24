# Post-capture audit — incomplete transport-stage diagnostic

The registered one-shot capture started at 2026-11-23 UTC and stopped after
approximately 53 seconds with exit code 2. No complete paired artifacts,
managed sidecars, or comparison artifact were produced. The output directory
contains only the incumbent and candidate receipt journals for the completed
`heatherquay_register` pair; the next receipt for
`conversation_control::wrenfrost_archive::wrenfrost_archive-fact-recall#1` was
rejected before it was appended.

The runner reported that attempted-turn, tool-call, repair-turn, clarification,
or token counters disagreed with the trace. This makes the capture invalid for
all frozen quality gates. It is not a model NO-GO, a memory-quality result, or a
treatment-effect estimate. The partial receipts retain only normalized traces,
hashes, and numeric metadata; they do not retain a model answer or provider
response content. The family IDs in this registration are exposed and must not
be reused. Do not rerun this capture.

Follow-up code inspection found a deterministic trace-contract defect: the
Chat Completions runner increments `user_clarifications` for a valid final
answer with `needs_clarification: true` but did not emit the matching
`clarification` trace event. A new public-runner regression reproduced this as
RED, and the minimal event emission now passes GREEN. The partial capture does
not contain the rejected `wrenfrost_archive` receipt, so this defect is a
plausible explanation for that error, not a proven attribution. A fresh
registration is required to test v5 stage telemetry after the regression fix.
