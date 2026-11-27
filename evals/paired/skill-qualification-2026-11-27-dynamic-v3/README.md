# Dynamic skill qualification — 27 November 2026

One frozen local capture: 16 unchanged prompts × previous/current skills = 32 trials. No remote inference or LM Studio server. Nine-turn, 384-token, 20-second request and 180-second trial caps; zero request retries. Seven native cases per arm (14 total) and nine controlled cases per arm (18 total) are reported separately. Controlled import/reembed are not real import or re-embedding of user data.

| Measure | All | Previous | Current |
| --- | --- | --- | --- |
| Full frozen semantic criterion | 8/32 | 3/16 | 5/16 |
| Final response observed | 14/32 | 7/16 | 7/16 |
| TimeoutError | 18/32 | 9/16 | 9/16 |
| Native full criterion | 5/14 | see per-case review | see per-case review |
| Controlled full criterion | 3/18 | see per-case review | see per-case review |

MCP full 5/16; CLI full 3/16. Summed trial elapsed 1,289.725 seconds is not a paired latency effect: initial capture overlapped Cargo, and the run was not an isolated performance experiment. The small descriptive 3→5 difference does not establish an instruction benefit. All 32 original trial digests were checked in independent semantic adjudication; CLI review used opaque case IDs before joining arms. MCP review was independent, not blinded.

`adjudicated-summary-v1.json` is the semantic result. `operation-regrade-v1.json` repairs two evaluator observation bugs on the same frozen outputs: completed operations remain observed after a final timeout, and an explicit documented default is accepted. Operation pass rises 10/32→16/32; full pass remains 8/32. Raw captures and original summary are immutable. The original CLI1 exact-sequence predicate rejects a legitimate linked readback; this conservative limitation is retained in frozen metrics. Both affected trials still lack a final answer.

The prospective `memory-mcp-skill-operations/v2` predicate now accepts only the authorized post-capture readback with the actual returned fact, episode and source binding. Wrong IDs/source/episode/order, extra mutations and failed readback are rejected by deterministic tests. This would change two historical operation flags to 18/32, not the frozen full outcomes or their registered protocol. Do not replace the published 16/32 with that prospective score.

The skill text already contains the omitted status/provenance/reference requirements. These failures distinguish instruction adherence, model deadlines, and evaluator limitations; they do not justify changing instructions to fit the observed answers or retrying until success. C2 remains NO-GO until fresh autonomous workflows qualify.
