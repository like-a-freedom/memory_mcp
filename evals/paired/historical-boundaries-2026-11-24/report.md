# Historical retrieval boundary checks

The native Apple Silicon integration tests observed the historical scan
boundary at 250 eligible facts. It returns all 249 or 250 eligible facts
without truncation. At 251 it returns 250 and reports truncation. All three
reads report partial completeness.

A full question about the capacity of Copper Warehouse missed an older
declared fact beyond 250 newer distractors on the incumbent. The short
subject query retrieved the fact at the same valid time. The candidate
uses the existing revision-visible lexical selector for a bounded subject
lookup when the original search has no admitted facts and its scan was
truncated. It admits a matching declared subject only if the fact also
matches the topic of the question. Existing successful reads retain their
path and ranking.

The wire regression captured a revision, repeated the exact revision
request before and after a backdated write, and observed identical items
and context metadata. The later source was visible at the newer revision.
The implicit latest-knowledge request and the explicit revision request
can echo different `known_at` values for the same revision, so the replay
compares two explicit revision requests.

An independent identity check reproduced a case-sensitive admission gap:
with declared subjects `Copper Warehouse` and `Copper Warehouse West`,
a question ending in `COPPER WAREHOUSE` returned both sources. This gap
requires its own test and acceptance slice. The bounded search candidate
keeps the existing subject identity policy.

The public MCP corpus and acceptance thresholds are registered in
[the historical boundaries corpus](../../corpora/memory-decisions-historical-boundaries/acceptance.json).

## Replay

Build the incumbent and candidate with the same locked features. Run the
following command twice per binary with a fresh absolute artifact directory
each time. Use the actual product revision for each executable.

```sh
MEMORY_DECISIONS_SERVER_BIN="$binary" \
MEMORY_DECISIONS_ARTIFACT_ROOT="$capture" \
MEMORY_DECISIONS_PRODUCT_REVISION="$revision" \
target/debug/memory-eval run \
  --profile evals/profiles/memory_decisions_historical_boundaries.json \
  --artifact "$capture/run.json"
```

Compare the two indexes per binary using `compare-decision-replays`, then
provide all four to `accept-memory-candidate` with the registered corpus and
acceptance file. The runtime and response size observations are descriptive.

## Acceptance result

Both repeats matched their respective first runs on native Apple Silicon.
The [acceptance verdict](verdict.json) recorded one family win (`old-subject`),
zero losses and zero hard failures. Family success rose from 1/3 to 2/3.
The two focal reads went from 0/2 to 2/2. Completeness remained partial.
The focal compact JSON response grew from 679 to 1316 bytes when it
returned the missing item. The three families establish the registered
regression result. The descriptive gain interval includes zero.
