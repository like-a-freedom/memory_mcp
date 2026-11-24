# Invalid run 01 — setup failures

This directory preserves the first attempted run and is **not a scored paired
comparison**.

- It was launched inside the restricted Codex tool sandbox. The 8 incumbent
  and 8 candidate receipts all report `model_failure`, zero model turns, and
  zero input/output tokens. A separate one-turn smoke reproduced the cause:
  the sandbox denied access to the authenticated local Codex state database
  and in-process app server. The same smoke succeeded when run through the
  user-authorized local unsandboxed path.
- The run then stopped when the source-file stratum began. The runner staged
  operator import fixtures into Codex's working directory, which the runner
  correctly requires to be empty before starting an agent. No file-import
  task completed.
- These receipts are retained as failures and excluded from any comparison.
  The plan, task manifest and its preregistration remain unchanged.

The next valid invocation requires both the unsandboxed local Codex execution
path and separate evaluator-only source staging. Those are runner execution
conditions, not changes to the memory treatment or task oracle.
