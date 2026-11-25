# C1 agent-managed correction recovery — v3

Pre-capture registration for the bounded partial-read recovery workflow. The v2 NO-GO remains immutable. This v3 pilot uses the new `agent_maintenance_v6` prompt profile and a portable host policy: the per-run CLI executable path is supplied by the user/runtime while its version and SHA-256 are pinned in the plan.

## Frozen question

Can the agent apply one user correction safely, then retrieve and answer the corrected fact in four fresh synthetic families? The evaluator has no retry-exposure floor; it rejects any retry that lacks exact authorization or changes non-query arguments. Zero retries can pass if the task gates pass. A private diagnostic reports exact first-query matches without recording raw query strings.

## Local execution boundary

- Native Apple Silicon only; direct local Codex CLI, no LM Studio.
- Remote inference, if the pinned Codex login uses it, receives only this registration's synthetic tasks and fixed prompts.
- Codex gets a fresh temporary `HOME` and trial workspace; the locally authenticated `CODEX_HOME` is used only for authentication with user config ignored. Host skill discovery and `skill_search` are disabled.
- Before registration, a PONG-only synthetic smoke confirmed the pinned CLI returned successfully without mentioning the personal skills directory or reporting skill metadata diagnostics.
- One capture per arm/family. Capture 02 is allowed only after capture 01 passes every deterministic gate.
- No real user, project, customer, or production memory is included.

The complete preregistration, frozen inputs, local run command, deterministic evaluator, and post-capture report are linked in this directory. Capture results are not yet present.
