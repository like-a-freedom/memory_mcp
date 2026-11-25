# C1 operator-import local qualification screen

**Status:** frozen before model task capture on 2026-11-25.

## Question and scope

On four new synthetic source-file tasks, can the provider-neutral local
Chat Completions runner finish both supported workflows: literal agent ingest
and operator import followed by read-only agent verification? Both arms use
the same current memory binary. This is workflow qualification only; it does
not estimate a memory treatment effect or a population success rate.

## Input equivalence

`input-equivalence.json` freezes one UTF-8 source sentence per family. The
sentence appears verbatim in the literal-ingest prompt and the candidate
operator file content has the identical bytes. The pre-capture check requires
the source strings and SHA-256 values to match for all four families. The
operator setup reads only the registered fixture; the candidate agent prompt
allows recall and prohibits writes.

## Frozen identity

`plan.json`, `tasks.json`, `fingerprints.json`, `sampling-config.json`,
`server-config.json`, `model-manifest.json`, `criteria.json`, and this file
define the cohort. `freeze-manifest.sha256` is the checksum root. Four families
are the independent units; the two arms and any repeat are correlated.

The runner and memory server execute locally on native Apple Silicon. The
OpenAI-compatible API is numeric loopback only. The model is served directly
by `mlx-lm.server` 0.32.2 with Hugging Face offline mode; no LM Studio process,
CLI or service participates. The weights directory's `.cache/lm-studio`
component is merely its current storage path. All task content is synthetic.

## Gate and stop rule

The first capture is a screen. It must have at least 3/4 exact task successes
in each arm, 4/4 independent evidence probes in each arm, 4/4 candidate
operator imports, zero unsupported/prohibited sources, zero hard violations,
and no resource or timeout exits. If any gate fails, do not run capture 02.
If it passes, run one repeat on the same four families only for workflow
repeatability; it does not add independent families. Do not change prompts,
task inputs, model, sampling, binaries or gates after the first task request.

## Measurements

Preserve task and write success, source coverage, independent probe outcomes,
operator import/extraction status and elapsed time, agent turns/tool calls,
token usage, hard violations, and wall time. Include setup cost in the report.
Do not claim that operator import improves memory quality: the two arms use
the same product and differ only in how the fact is formed.
