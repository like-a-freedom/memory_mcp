# Pre-capture audit — Ornith transport-stage diagnostic

Completed before the first model-task request for this registration.

## Checks

- The registered family IDs (`wrenfrost_archive`, `heatherquay_register`,
  `cedarmist_terminal`, and `ambercross_locker`) were searched against
  4,767 prior JSON/JSONL `family_id` occurrences under `evals/`.
  No overlap was found.
- `memory-eval hash-chat-completions-tasks` produced the corpus, oracle,
  prompt, host-policy, retry-policy, protocol, simulator, endpoint and sampler
  fingerprints recorded in `fingerprints.json`; those values are copied into
  `plan.json`. Task keys match the four preregistered families and strata.
- Product binary SHA-256: `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
  Product configuration SHA-256: `dd189d6af56cb77dbb6c33c0f9e1a4331b15a7b260f96a53c79b698650508a12`.
  Evaluator CLI SHA-256: `a0143edca3d44f63b76122a34523eb4b36296c1b663b1ebad1b441349ad33b11`. The product and evaluator binaries match
  the plan.
- `lms ps --json` reports `memory-local-eval-ornith-v2` loaded and idle with
  context length 16,384 and one parallel request. `lms --version` reports CLI
  commit `dcefd5a`; `/v1/models` returned HTTP 200. The provider-neutral local
  smoke test completed exactly one public `assemble_context` call and passed
  in 1.97 seconds. Only content-free smoke fields are retained.
- The endpoint is numeric loopback and no bearer token or external service is
  configured. Only the four preregistered synthetic fact families are used.
- The earlier HTTP-status diagnostic, corrected-sampler screen, and all other
  exposed cohorts remain immutable. None of their family IDs is reused.
- New sidecar-v5 fake-endpoint, redaction, schema-compatibility, and transport
  stage validation tests passed. The full `eval-harness` suite passed
  (770 passed, 23 ignored), strict workspace Clippy passed, and formatting and
  diff checks passed before this registration was frozen.

## Limits

These checks establish disjoint inputs, exact hashes, current product/evaluator
binary identity and loopback tool compatibility. They do not establish agent
quality, memory improvement, or a treatment effect. The capture records a single
model configuration for diagnosis; apply the one-capture limit and frozen gates
in `preregistration.md` and preserve every emitted artifact, including a no-go
or inconclusive outcome.
