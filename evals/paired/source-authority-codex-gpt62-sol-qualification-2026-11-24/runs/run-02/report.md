# GPT-6.2 Sol source-authority workflow qualification — run 02

**Result: PASS for the preregistered model/workflow screen.** This is a narrow
qualification of Codex CLI 0.171.0 with the requested `gpt-6.2-sol` alias on
four synthetic families. It is not a memory-product comparison, a powered
agent-benefit study, or a population estimate.

## Frozen gate and result

The valid capture completed on 2026-11-24 from 04:57:03.228911Z to
05:02:26.000477Z (322.772 seconds). It used four newly registered synthetic
families, two conversation controls and two source-file-import cases. The
frozen input manifest verified before capture; the invalid run-01 preflight is
retained separately and contains no trial.

| Preregistered condition | Incumbent | Candidate | Gate |
|---|---:|---:|---|
| Task-oracle success | 4/4 | 4/4 | At least 3/4 in each arm: pass |
| Agent-observed direct-memory success | 4/4 | 4/4 | Descriptive; zero paired delta |
| Independent fixed-query probes | 4/4 | 4/4 | 4/4 in each arm: pass |
| Source-file task answers | 2/2 | 2/2 | Candidate 2/2: pass |
| Candidate operator imports and extraction | — | 2/2 each | Both 2/2: pass |
| Hard violations | 0 | 0 | Zero: pass |
| Budget-exceeded trials | 0 | 0 | Zero: pass |

The paired task-success and agent-observed direct-memory deltas are both zero.
Both arms used the same memory server binary, product configuration and tool
schema (`memory_mcp` SHA-256
`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`). Thus
the capture cannot estimate a memory-product treatment effect.

## Descriptive workflow measurements

- Incumbent formation: 159,121 input tokens and 8 formation tool calls.
- Candidate formation: 135,012 input tokens and 6 formation tool calls.
- Candidate operator setup: two synthetic file imports and two extractions,
  four setup actions total, 990 ms measured setup time.

These are raw measurements from four families. They suggest that the
operator-import workflow shifted some work out of the model session; they do
not establish a statistically reliable reduction in total user effort. The
setup actions must be counted when comparing workflows. The separate
independent probes passed for every trial, so their success must not be
confused with an agent-observed recall gain.

## Integrity and reproducibility

- Plan: `plan.json`, SHA-256
  `40fc5bf50e85273acd886a6deae9d91343abad9bf3445f291238d791b7375cee`.
- Product binary SHA-256:
  `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`;
  Codex CLI SHA-256:
  `fc8d77cf7585aded2b9bc487dad29c4ee2f066123e4e76dd3b17653a8322ce42`;
  `memory-eval` SHA-256:
  `c9299fdf3425f4b356784e027020407689cbdac2c0c32bf6d8a6248aee89f533`.
- Incumbent and candidate artifact SHA-256 values are recorded in
  `agent-managed-comparison.json` and the frozen plan.
- Offline comparison replay is byte-identical: both
  `agent-managed-comparison.json` and
  `agent-managed-comparison-replay.json` have SHA-256
  `682830b8869deb245f240b7f1ac95661c174a925bf1b05ee524266044dbb7f97`.
- The frozen registration inputs still match all entries in
  `freeze-manifest.sha256`.
- Remote model input was restricted to the registered synthetic prompts,
  synthetic memory responses and tool schemas, as authorized. No real user
  content was supplied. The runner retained normalized results and digests,
  not raw final answers.

See [`preregistration.md`](../../preregistration.md) for the frozen question,
scope and gate; [`execution-addendum.md`](../../execution-addendum.md) records
why run-01 was an invalid preflight and how to reproduce run-02.

## Decision

This result qualifies this exact Codex CLI/model-alias workflow for a future,
separately preregistered study. It does not satisfy a broad source-authority
usability claim or the powered agent-benefit requirement. A product-benefit
study still needs a real treatment contrast, an impact margin justified for
that contrast, sufficient independent families, and a joint power plan. No
such contrast is present in this qualification.
