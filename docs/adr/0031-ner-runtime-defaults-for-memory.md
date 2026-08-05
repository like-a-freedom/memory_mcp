# 0031: NER Runtime Defaults for Memory

## Status
Rejected

## Context
Windows are capped at 384 tokens (gliner_config.json `max_len: 384`).
`run_forward_batch` pads each batch to the LONGEST window in it, not to
NER_MAX_BATCH_TOKENS; that value only bounds how many windows are packed
into one batch (pack_window_batches, gliner.rs:1239).

## Decision
Do NOT change DEFAULT_NER_MAX_BATCH_TOKENS. At the default batch_size=1
(and the user's config), every batch is a single window, so max_batch_tokens
never binds and lowering it changes nothing about memory. It would only
reduce window packing for batch_size>1 — a throughput regression with no
memory benefit. Activation memory is already sized to the actual window
(<=384 tokens); the 1536 default is a packing ceiling, not a padding target.

## Consequences
+ No change; the padding knob was a red herring (verified in run_forward_batch).
- Future work on activation memory must target the forward-pass itself
  (e.g., Metal/Accelerate per the latency plan), not this constant.
