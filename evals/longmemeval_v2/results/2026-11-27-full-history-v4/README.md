# Full-history admission candidate v4

Registered seven-case replay on separate clones of the restored 100-history, 151-episode store, using unchanged queries, original span assertions, budget 5, 30-second read and 45-second process caps. One candidate capture, no retries. The source snapshot was not opened for mutation and its file digests remained unchanged.

Six contexts delivered exactly the same source text as v3. Original source/span coverage remains 4/7; independent delivered-source sufficiency remains 6/7. Two sufficient alternate sources remain strict gold misses. `922a0c56` again delivered no context: external process cap at 45.118 seconds, exit -9. Candidate deadline acceptance FAILED; see `candidate-decision.json`. The native gates separately passed 8/8 temporal and 37/37 retrieval with all original floors and zero invalids.

Lazy construction removes demonstrably unused raw index-key views. Actual dictionary builds for seven cases, v3→v4: 228→224, 296→294, 302→298, 302→298, 2→2, 298→296, 300→296. This is a finite allocation-work correction, not evidence that vocabulary construction alone fixes the timeout. Broad repeated-topic Experience candidates legitimately still require both views.

Single-case process and assembly observations are retained in `reads/`. They do not establish a causal paired speed gain: engine/cache variance is unmeasured and the v3 idle model residency differs from v4. The follow-up phase diagnostic is a separate run, not a replacement for this failure.
