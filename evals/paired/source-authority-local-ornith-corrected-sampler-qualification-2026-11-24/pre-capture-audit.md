# Pre-capture audit — 2026-11-23

This audit was completed before the first model-task request. The registered
screen remains a single capture; do not rerun it or change any frozen input
after capture starts.

## Checks completed

- The four registered family IDs (`quillforge_registry`,
  `opalwharf_archive`, `foxglove_pier`, and `larkspur_shed`) were searched
  against 3,760 `family_id` occurrences in JSON artifacts under
  `evals/paired/`, excluding this cohort. No overlaps were found.
- `memory-eval hash-chat-completions-tasks` reproduced the registered task,
  corpus, oracle, prompt, protocol, retry, simulator, endpoint and sampler
  fingerprints. The output agrees with `fingerprints.json` and the matching
  fields in `plan.json`.
- The local product binary SHA-256 is
  `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`;
  the evaluator binary SHA-256 is
  `7ff353b41d5cb773fefe36cb804738fa2b72303efbac0fc3bf5b46168db5146e`.
  Both agree with `plan.json`.
- `memory-local-eval-ornith-v2` is present in LM Studio's active model list
  and in the `/v1/models` response. The local server is bound to port 1234.
  `lms ps` reports the alias idle, with a 16,384-token context and one
  parallel request. The LFM2.5 model is available in the server inventory but
  is not currently loaded in the active process list.
- The endpoint is numeric loopback (`http://127.0.0.1:1234/v1`). The four
  cases contain synthetic facts. The preflight receipt contains only
  content-free protocol evidence; it is not included as task-quality evidence.
- The existing LFM2.5 pilot and a disjoint diagnostic both failed their
  registered task floor. Their exposed cohorts are not reused here. The
  current Ornith sampler is frozen to the model card values
  `temperature=0.6`, `top_p=0.95`, `top_k=20`; the previous Ornith cohort is
  not reused.

## Scope of evidence

These checks establish cohort separation, artifact consistency, binary
identity, and local endpoint/model availability. They do not establish model
quality, memory improvement, provider behavior outside the local endpoint,
model-weight hashes, or LM Studio server build identity. Apply only the gates
in `preregistration.md` after the one capture and offline comparison.
