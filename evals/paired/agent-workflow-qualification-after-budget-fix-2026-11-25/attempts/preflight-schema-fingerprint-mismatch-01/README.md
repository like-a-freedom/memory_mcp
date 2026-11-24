# Invalid preflight: product schema fingerprint mismatch

The first attempt used the frozen v1 registration and stopped before the agent
backend ran. The probe capture was rejected with
`independent capture provenance mismatch`; both receipt journals are empty.
The MLX server log records only `GET /v1/models`, with no chat completion
request. Therefore no model saw a task, and no trial or metric was produced.

Audit found a transcription error in v1's `tool_schema_sha256`: it differs from
the same current product fingerprint in the prior checked registration. The
v1 plan and freeze manifest remain unchanged. A corrected v2 registration was
frozen separately before any model request; its input families remain
unexposed to the model.
