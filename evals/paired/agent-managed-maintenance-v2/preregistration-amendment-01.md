# Preregistration amendment 01 — host execution failure

Recorded 2026-11-22 after the initial v2 runner attempts and before any valid
v2 model turn.

The frozen v2 task, product, model, scoring, response cap, budgets, prompts and
profile remain unchanged. The initial captures in `run-01` and `run-02` are
preserved but excluded as **invalid host preflight attempts**: every receipt
has `model_failure`, zero turns, zero input/output tokens, zero tool calls and
no completed writes. The post-write probes therefore failed because the
correction was never attempted. They are not feasibility observations and do
not establish a 2048-token effect.

A read-only diagnostic of the same local Codex CLI stopped before its first
model turn with `Operation not permitted` while initializing Codex's in-process
app-server and local state under the outer workspace sandbox. The local CLI
reports an existing ChatGPT login; the pinned model alias appears in the
installed model catalog; the official configuration reference defines the
per-tool output limit as a positive integer. The failure was host sandbox
access, not an observed model or memory outcome. No credential or Codex state
was copied into the repository.

The user had explicitly authorized unsandboxed execution on this laptop. Two
valid repeats will therefore use the same frozen plan and inputs in `run-03`
and `run-04`, with direct host launch. The child Codex process keeps its
read-only sandbox; shell, filesystem, web, plugins, apps and Codex memories
remain disabled. Its memory MCP allowlist and all trial mutations remain
restricted to the evaluator-created temporary store. These runs are the only
v2 feasibility repetitions. The failed `run-01` and `run-02` directories stay
archived for audit but are not included in scoring.
