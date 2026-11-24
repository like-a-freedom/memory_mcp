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
