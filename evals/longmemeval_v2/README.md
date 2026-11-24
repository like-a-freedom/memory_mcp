# LongMemEval-V2 Adapter

> Official LongMemEval-V2 adapter contract for `memory_mcp`.

## Pinned revisions

```text
LongMemEval-V2 repository commit:
  ef67f10aacd9080c75aeb2dd527a0af25dc26f1b

Hugging Face dataset revision:
  f152293e235517d504809563c833d7190b8c713b
```

The previous adapter-smoke pin was `6f020ac2fc3275e46c706d3406e02c3ed79b7be2`. The current
pin passes only an opaque invocation ID in backend query context. The
dataset revision is retained. See the [upstream change](https://github.com/xiaowu0162/LongMemEval-V2/commit/ef67f10aacd9080c75aeb2dd527a0af25dc26f1b).

The local smoke exercises the adapter only. Official scoring still
requires the adapter to be registered in the upstream process before
configuration is loaded, as well as a frozen dataset cohort, storage
isolation, and the approved reader and judge configuration.

## Backend interface

The adapter implements the official memory backend interface:

```python
class MemoryMcpBackend:
    def insert(self, trajectory):
        ...

    def query(self, query, query_image=None):
        ...
```

The adapter invokes the existing public `ingest`/inline `extract` and
`assemble_context` surfaces or the protocol-agnostic equivalents. It does not
seed facts directly because that would bypass memory formation and invalidate
the benchmark.

## Coverage

- **Text-capable Small tier:** run only examples whose trajectory and query do
  not require image understanding. Report coverage and never label the subset
  as the full benchmark.
- **Full Small tier:** remains unsupported until image content and
  `query_image` have an explicit representation, retrieval, and evaluation
  design.
- **Medium tier:** run only after Small passes capacity, ingest-time, and
  query-latency budgets.

LongMemEval-V2 reports five abilities separately:

- static state recall;
- dynamic state tracking;
- workflow knowledge;
- environment gotchas;
- premise awareness.

## Limitations

Do not copy an external leaderboard percentage into the release gate. Compare
the same pinned harness before/after and publish limitations. A text-only
LongMemEval-V2 subset is **not** the full benchmark.

## Python isolation

Python dependencies remain isolated from the Rust runtime. The adapter runs
out-of-process and communicates via the public MCP/CLI surface.

## Verify the upstream query contract

Check out the repository at the commit above. In a Python environment with
the upstream requirements and `httpx` installed, run its privacy tests
from the pinned checkout. The registry imports backends that require
`httpx` directly.

```sh
PYTHONPATH="$upstream" python -m unittest discover \
  -s "$upstream/tests" -p test_query_privacy.py
```

On native Apple Silicon, the seven upstream privacy tests, the four
adapter tests, and the local CLI integration smoke passed. Importing the
adapter alongside the actual pinned registry also resolved
`memory_type=memory_mcp` and exposed only the opaque invocation ID in query
context.

## Independent reads

Each query copies the configured store while the adapter holds its store lock.
The product reads and updates access counters in the temporary copy, which is
removed at the end of the query. This prevents one question's reads from
changing the next question's ranking inputs. The pinned upstream smoke tested
two questions in both orders after restoring the same snapshot. A native
retry smoke verified that restarting the adapter and replaying the same input
preserves the retrieved context.

Snapshot copy time is included in query time. Budget the store size, extra
disk space and copy throughput before official scoring. Snapshot isolation
controls persisted read effects. Separately qualify the product clock and
the complete input and scorer configuration. A successful no-hit read
returns an empty list. A malformed or blank context item fails explicitly.

## Small dataset preflight

The [expected hashes](dataset-small.sha256) are from the pinned release.
The question and Small haystack files matched their expected SHA-256
values in a fresh download. The question IDs match the haystack keys
exactly. All 240 web questions share one ordered 100-trajectory haystack.
All 211 enterprise questions share a different ordered 100-trajectory
haystack. Of those questions, 225 web and 197 enterprise have a null
`image` field. They require per-question evidence adjudication before
inclusion in the text-capable cohort. [Pinned schema](https://huggingface.co/datasets/xiaowu0162/longmemeval-v2/blob/f152293e235517d504809563c833d7190b8c713b/SCHEMA.md)

A synthetic single-question build also passed through the pinned
`evaluation/harness.py` with the adapter registered in-process and
`--save-memory --skip-evaluation`. The first restore failed after the original store was moved. The adapter
now snapshots the quiescent embedded store and restores into a fresh private
working directory. The pinned upstream save/restore smoke passed on Apple
Silicon after moving the original store, using the expected retrieved fact
as its oracle. An existing destination is refused and the snapshot can be
loaded independently of the original storage path.

To repeat the optional upstream smoke, point `PYTHONPATH` at the pinned
upstream checkout and this repository, and set
`LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY` to the native product binary. Run
`python -m unittest evals.longmemeval_v2.test_upstream_restore` with the
upstream test requirements available.

For a build-only run with a shared ordered haystack, use the same pinned
checkout and a fresh output directory:

```bash
python -m evals.longmemeval_v2.build_preflight --domain web \
  --questions /path/to/questions.json --haystack /path/to/haystack.json \
  --trajectories /path/to/trajectories.json --binary target/debug/memory_mcp \
  --output /path/to/new-output
```

The runner writes `build-coverage.json` with all selected question IDs.
A failed product ingest or extract returns exit code 1 and attributes the
aborted build to every dependent question. A successful snapshot returns 0.
Both outcomes report zero scored cases and unavailable official metrics.
Invalid inputs and other execution errors surface directly. The preflight
uses no scorer, credentials, or retry.

Pass `--formation-method ordered-segments-v1` to exercise bounded
memory formation. This method uses public ingest and extract for each
segment in order. It prefers boundaries between states and splits an
oversized state at a UTF-8 scalar boundary. Concatenating the segment
bodies reproduces the canonical trajectory byte for byte. The source ID
includes the whole trajectory digest, method version and segment ordinal.
The method forms multiple episodes when required, so cross-segment
extraction and answer quality must be evaluated for this method separately.
The default `whole-trajectory-v1` remains available for comparison. Build
receipts and saved configuration record the selected method.

Streaming the pinned trajectories verified the third file hash and
resolved the 200 distinct Small trajectory IDs. The current JSON
serialization reaches 7,600,069 bytes for one trajectory. In total,
53 trajectories exceed the native Apple Silicon process argument
budget of 1,048,576 bytes on content alone. A synthetic argument of the
maximum size was rejected by macOS with `E2BIG`. The adapter now sends
the complete UTF-8 body via `ingest --content-stdin` with an 8 MiB limit.
It requires an explicit dedicated embedded store and disables embeddings
in an allowlisted subprocess environment. A digest of canonical JSON and
the undated-trajectory surrogate `2000-01-01T00:00:00Z` form the input
identity across retries. The longest pinned trajectory (7,600,069 bytes) reached durable ingest on
native Apple Silicon. The subsequent extract refused the whole input at
the default Anno limit of 1,048,576 bytes. The adapter keeps this failure
visible and preserves the product limit. In a synthetic test of the
actual upstream harness, the same class of failure aborted a shared build
for two questions before a snapshot, prompt rows or score were emitted.
[Native aggregate](dataset-small-native-preflight.json)
