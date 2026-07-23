# LongMemEval-V2 Adapter

> Official LongMemEval-V2 adapter contract for `memory_mcp`.

## Pinned revisions

```text
LongMemEval-V2 repository commit:
  6f020ac2fc3275e46c706d3406e02c3ed79b7be2

Hugging Face dataset revision:
  f152293e235517d504809563c833d7190b8c713b
```

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
