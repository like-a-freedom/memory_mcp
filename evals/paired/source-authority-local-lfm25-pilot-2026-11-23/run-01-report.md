# Run 01 report — LFM2.5 feasibility NO-GO

**Plan:** `source-authority-local-lfm25-1.2b-2026-11-23-pilot`  
**Plan SHA-256:** `32e94e2053506edabafd1d2c01a0ee5d0855813c72140c12eec91befef1b47e3`  
**Capture:** `5903-1795428967161218000-0`  
**Run time:** 73.88 s wall-clock  
**Task-oracle success:** incumbent 0/4, candidate 0/4  
**Paired comparator:** accepted; 4/4 families, zero hard violations, zero budget-exceeded trials

The provider-neutral runner connected to the local LM Studio endpoint and
emitted valid, hash-bound artifacts. All eight arm/family receipts ended in
`model_failure`; task-oracle success was zero in both arms. Formation had one
completed phase and three model failures in each arm. The candidate's fixed
independent probes passed 2/4 after operator imports; incumbent probes passed
0/4. This is expected product-state evidence from the operator setup, not
evidence that LFM completed the agent task. The direct-memory paired estimate
was +0.50 with a four-family descriptive interval [0, 1]; the task-success
estimate was 0.00. Neither supports a model-quality or product-benefit claim.

The eight recorded trial wall times summed to 5.138 s (mean 642 ms, max
980 ms). The full process took 73.88 s, including isolated product/database
setup and evaluation overhead. This confirms that model generation itself was
fast on this laptop, while the complete paired trial still has substantial
non-model overhead. Do not compare this four-family failed pilot directly to
the 16-family Qwen capture as a matched speed benchmark.

The earlier synthetic fake-MCP smoke passed after its task rules explicitly
required string episode IDs in the citations array. Its initial underspecified
rule produced a citation object and failed response validation. The production
recall task rules already pin the required string-ID schema. That smoke remains
transport/format compatibility evidence only.

**Decision:** NO-GO for replacing Qwen with LFM2.5 as the agent model for the
registered source-authority scenarios. Keep LFM for fast transport and
tool-call smoke checks. Continue the frozen Qwen v3 repeat without changing
prompts, tasks, product, or evaluator. A future LFM workflow attempt requires
a new disjoint registration and should first isolate the source of its
`model_failure` results with a separate synthetic diagnostic; do not retry
these four families.

Artifacts: [`run-01`](runs/run-01/), including receipts, both managed sidecars,
and the accepted `agent-managed-comparison.json`. Comparator exit status was
0. Frozen inputs remain verifiable with the adjacent
[`freeze-manifest.sha256`](freeze-manifest.sha256); run outputs are excluded
from that input manifest.
