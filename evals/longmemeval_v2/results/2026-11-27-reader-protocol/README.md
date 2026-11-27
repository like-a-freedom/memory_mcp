# Reader protocol experiment, 27 November 2026

One preregistered local capture; seven archived contexts, three declared variants,
256-token completion cap and temperature zero. No retrieval, formation, external
inference, or changes to the historical capture. Standalone MLX serves already
cached weights via a replaceable OpenAI-compatible numeric-loopback endpoint.

`registration.json` binds input/truth/runner hashes and the stopping rule.
`prompts.json` contains actual registered prompts; `projection.json` preserves each
original item index, raw digest and character-to-raw offsets for the one-pass
readable projection. It does not add or select evidence. Model source IDs remain
inside the preserved text; the item index is not an asserted episode identity.
`inference-attempts.jsonl` records transport attempts including faults; unknown
provider timings/usage stay null. First observed request is not verified cold state.
Server startup was not timed; do not subtract it or claim startup improvement.

The new protocol requires literal boxed answers and exact quotes. For a false
premise it requests `None` plus explanation; `None` was already in the frozen
accepted-answer contract for these three cases. This is an explicit reader
protocol intervention, not an upstream official score or new accepted answer.

`report.json` separates value, refusal, finish_reason, formatting, literal quote
presence and unobserved/error counts. A literal substring quote does not establish
source-bound citation correctness, completeness or support; Task 6 evaluates those
separately. Strict accepted-answer equality can miss unregistered valid paraphrases.
Truncated outputs remain in the denominator and cannot pass completed_correct.
Fixed variant order and uncontrolled host load prohibit general speed conclusions.
The exposed development sample cannot qualify general benefit or C2.

Observed: 21/21 logical completions, 21 transport attempts, zero failed attempts; wall 281.432s. Each variant has value/completed accuracy 3/7, format 7/7 and refusal 0/3. The original scorer reported literal quote presence as 3/7, 4/7 and 5/7 respectively; this is not source-bound citation accuracy. No quality improvement demonstrated.

## Deterministic scoring erratum v2

`quote-audit-v2.json` corrects quote presence to **3/7, 3/7, 5/7** against each variant's actually delivered representation. The old scorer wrongly allowed the decoded projection for raw variants; final-first case `922a0c56` was credited for text never supplied in that form. Value3/7, refusal0/3, format7/7 and all original outputs are unchanged. The historical report and exact runner/observer `.source.txt` files remain immutable. The corrected score is an offline audit, not a new capture or improvement. New captures use the corrected grader and hardened no-proxy/no-redirect transport. The earlier frozen captures predate the durable start-event hardening; all21 attempts have observed completion rows, but no start journal or historical cancellation claim is retroactively fabricated.
