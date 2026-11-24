# Agent-managed memory maintenance feasibility pilot

Registered: 2026-11-22, before any live agent trial.

## Question and scope

Can a read-only-by-default Codex runner complete one bounded correction on an
existing memory by adding the user-supplied correction, extracting it, then
invalidating only the superseded fact with its valid-time boundary? This is a
feasibility and cost pilot. Both paired arms use the identical product binary,
configuration, Evidence stage, model alias and tool profile. It estimates no
memory treatment effect, population benefit, or non-inferiority.

## Frozen inputs

- Plan: `plan.json`; task and hidden-oracle manifest: `tasks.json`.
- Eight disjoint synthetic history families, one old durable fact and one
  user-provided correction per family. The correct new source is required and
  the old source is prohibited at the fixed post-write probe.
- Maintenance profile exposes only the existing `assemble_context`,
  `ingest`, `extract` and `invalidate` MCP tools. Shell, filesystem, web,
  plugins, apps and Codex memories are disabled. Recall starts in a fresh
  read-only agent session. The independent probe is a direct public MCP call
  after both agent sessions, against that trial's same isolated store.
- Codex CLI `0.171.0`, model alias `gpt-6.1-sol`, stdio transport, 65,536-token
  context budget, eight tool calls/turns per phase, 2,048 output tokens and
  120-second phase limit. Authentication uses the existing local Codex home;
  no API key is copied to artifacts.
- Product binary, evaluator binary, Codex executable, prompt, host policy,
  retry policy, protocol, simulator, task corpus and oracle fingerprints are
  pinned in `plan.json`, `fingerprints.json` and the adjacent SHA files.

## Trial and endpoints

Run two independent repetitions of all eight families. Each family runs once
per arm, in its registered order, with a fresh isolated store. Do not edit the
plan, task manifest, prompts, configuration, binaries or scoring rules after
the first observation. A failed preflight yields no trial; an agent-visible
failure after start remains in the denominator.

Primary feasibility endpoints, reported per family and per repetition:

1. Agent task success and fresh-session answer correctness.
2. Independent post-maintenance evidence success, required-correction support,
   prohibited old-source count, unsupported-source count and hard violations.
3. Episode-ID binding for exactly one ingest; successful ingest, extraction
   and invalidation counts. Any duplicate or unresolved operation is reported.
4. Completed phases within the preregistered token, call and time limits.

Secondary descriptive cost endpoints: maintenance-phase and recall-phase turns,
tool calls, input/output tokens, wall time, clarification count, and retained
store bytes. Store bytes are not process-memory usage. Report arm-level values
and the paired family results; do not turn a run-to-run difference between
identical arms into a product effect.

## Feasibility decision

The protocol is feasible only if both repetitions have complete coverage, all
32 maintenance actions and all 32 independent probes finish within limits,
all 32 task answers pass, each trial binds exactly one correction episode, each
trial succeeds with one ingest, one extract and one invalidation, and there are
zero unsupported/prohibited-source observations or hard violations. Otherwise
retain the run as a diagnostic and report the observed failure mode. These
criteria qualify the runner and this narrow workflow only; they are not a
candidate acceptance margin or an agent-benefit guarantee.

## Limits

The cohort uses eight shared synthetic templates, a single correction, one
fixed model alias and one local Codex host. The paired arms are the same
product. Success cannot establish long-lived maintenance quality, ease for
other models or users, non-inferiority after a product migration, or a memory
quality treatment effect. This pilot does not expose operator file/URL import.
