# Pre-capture audit — Ornith HTTP-status diagnostic

Completed before the first model-task request for this registration.

## Checks

- The registered family IDs (`birchglass_registry`, `tallowmere_ledger`,
  `cloudbarrow_pier`, and `mintvale_depot`) were searched against 4,637 prior
  JSON/JSONL `family_id` occurrences under `evals/paired/`. No overlap was
  found.
- `memory-eval hash-chat-completions-tasks` reproduced the corpus, oracle,
  system/task prompt, host-policy, retry-policy, reader/protocol, simulator,
  endpoint and sampler fingerprints stored in `fingerprints.json` and
  `plan.json`. The four new family keys match the plan exactly.
- Product binary SHA-256:
  `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
  Evaluator CLI SHA-256:
  `0de92979bc8b8cf761acf4faa4efb48debdbda9d051db8f78b441768d20be9bd`.
  Both match the plan.
- `lms ps --json` reports `memory-local-eval-ornith-v2` present and idle with
  context length 16,384 and one parallel request. `lms --version` reports CLI
  commit `dcefd5a`. The server is listening on port 1234; `/v1/models` returned
  HTTP 200. A short synthetic Chat Completions request returned HTTP 200, and
  the provider-neutral test smoke completed exactly one public
  `assemble_context` call. Only content-free smoke fields are retained.
- The endpoint is numeric loopback and this run configures no bearer token or
  external service. Synthetic facts only are registered.
- A scan of earlier JSON and JSONL paired artifacts found no family overlap.
  The previous corrected-sampler Ornith cohort is immutable and is not
  repeated. The available LFM2.5 and Qwen cohorts also remain exposed and
  failed their frozen gates; none are used to tune this registration.
- The new sidecar v4 and shared-wire compatibility behavior passed the full
  evaluator test suite and strict workspace Clippy before this registration.

## Limits

These checks establish disjoint inputs, frozen fingerprints, binary identity
and current local endpoint/tool compatibility. They do not establish model
quality, memory improvement, status-class behavior under the failed workflow,
or a treatment effect. Apply the one-capture limits and gates in
`preregistration.md`; preserve artifacts even if the status diagnostic is
inconclusive.
