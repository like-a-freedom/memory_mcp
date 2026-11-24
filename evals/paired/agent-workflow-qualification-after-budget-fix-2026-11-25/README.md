# Agent workflow qualification — tool-choice v1 preflight

**Disposition: invalid preflight; excluded from model outcomes.**

This registration attempted to measure whether setting `tool_choice: "none"`
after the one-call budget would prevent a second model tool call. Its frozen
tool-schema fingerprint did not match the registered schema, so the
independent-probe preflight rejected it before any agent completion request.
The local MLX server log showed only model-list discovery, not a task request.
The empty receipts are retained under
[`attempts/preflight-schema-fingerprint-mismatch-01/`](attempts/preflight-schema-fingerprint-mismatch-01/)
and must not be counted as a model failure or success.

The preregistration and original frozen inputs are preserved byte-for-byte.
The diagnostic was corrected by creating a fresh v2 registration with the
verified tool-schema fingerprint and disjoint synthetic family names.
