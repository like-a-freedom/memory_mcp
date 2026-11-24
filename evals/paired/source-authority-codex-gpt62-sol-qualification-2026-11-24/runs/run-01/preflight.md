# Run 01 — invalid technical preflight

The harness rejected the invocation before allocating a trial workspace because the registered `--temp-root` did not yet exist. In the runner, `TrialDirectory::create` checks the root before seeding the memory server or invoking Codex. The only files already created in this run directory were empty incumbent/candidate receipt journals.

- Agent trials started: 0
- Codex agent invocations: 0
- Model tokens / memory tool calls: 0 / 0
- Qualification capture consumed: no
- Classification: invalid preflight, excluded from all scores

Preserve this attempt. The single valid capture is run 02; no registration inputs, task families, prompts, model alias, limits, or gates changed. See [`execution-addendum.md`](../../execution-addendum.md).
