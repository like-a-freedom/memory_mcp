# Run 01 report — invalid before model turn

**Plan:** `source-authority-codex-literal-input-2026-11-23-qualification`  
**Run:** `5756-1795448755783563000-0`  
**Status:** invalid technical startup attempt; excluded from qualification scores.

All eight arm receipts ended as `model_failure` before a model turn: zero turns,
zero tool calls, and zero input/output tokens. The Codex child exited while
initializing its in-process app-server with `Operation not permitted` inside
the execution sandbox. A separate one-message synthetic smoke succeeded when
Codex ran outside that sandbox, confirming this was a process-permission issue
and not a model response or memory result.

The registration, tasks and this attempt are preserved unchanged. Since no
model turn or model tokens occurred, this is not a valid capture and does not
consume the registration's single qualification capture. The first valid
capture is preserved as [run 02](../run-02/report.md). Do not interpret the
zero-success summary emitted by the runner as a task-quality result.
