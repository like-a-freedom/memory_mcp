# Offline workflow evaluation replay — 2026-11-26

No model server, Codex process, provider, or memory server was invoked for these replays. The commands recomputed qualification reports from saved synthetic artifacts only.

## Ordinary agent-control v3

The provider-neutral `qualify-agent-workflow` command returned **ELIGIBLE**. Both captures still meet 4/4 task, direct-read, and independent-probe criteria with zero hard violations or budget exits. The report is a process-control qualification for that exact workflow configuration; it is not a memory treatment comparison.

## C1 correction recovery v7

Both the Rust `compare-agent-managed` and frozen Python `evaluate_c1.py` outputs were regenerated for captures 01 and 02. Recovery validators were also rerun on the paired artifacts.

- `ordinary-qualification.json` is byte-identical to its archived report (`98f50b1a5797dddb57e09f17d5038dbb3584decfe7cbafbe0c277d147cf3b522`).
- `c1-capture-01-comparison.json` is byte-identical to its archived report (`b44e36372215f8b8519da416641e17da6d59d8a27eab14ecb519695d08e01318`).
- `c1-capture-02-comparison.json` is byte-identical to its archived report (`1b28270367de77da0f95620ff404cc71345045ec605325260a0ba18d9121e0ca`).
- `c1-capture-01-evaluation.json` is byte-identical to its archived report (`d58b64b04204049bba3e65d44af718d63336476ed598f083202255d904dff039`).
- `c1-capture-02-evaluation.json` is byte-identical to its archived report (`a0bec881c26f7cdfbaec45d7909eb8b0467785df750872e192858fcdba2f3ef0`).

Capture 01 remains PASS (4/4 exact cited tasks per arm). Capture 02 remains NO-GO (0/4 per arm; one candidate prohibited-evidence violation). The recovery contract passes in both captures; capture 02 shows three authorized retries per arm and zero retry-contract violations. The preregistered repeat is exhausted; no third capture was run.

These are offline artifact replays and do not reproduce the model interactions. The cross-capture interpretation and selector diagnostics remain in the [frozen C1 review](../c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/CROSS_CAPTURE_REVIEW.md).
