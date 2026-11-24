# Local Ornith HTTP-status diagnostic

This one-capture, four-family diagnostic checks whether the earlier Ornith
`transport` formation failures were non-success HTTP responses. It uses the
same model configuration and prompts as the corrected-sampler screen, but four
new synthetic families. The updated agent-managed stage sidecar v4 records only
a numeric HTTP status when the provider rejects a request; it never records
response headers or body. Both arms use the same product binary, so this is not
a memory treatment comparison.

The exact setup, limits and gates are frozen in
[`preregistration.md`](preregistration.md). The local server and synthetic
tool-call smoke passed before this registration was frozen. Deterministic tests
and offline scoring do not require LM Studio or an API key.

Verify every frozen input from the repository root before the one capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/freeze-manifest.sha256
```

The preregistered capture command is:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01"
```

After the capture, compare the normalized artifacts offline:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- compare-agent-managed \
  --plan evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/plan.json \
  --incumbent evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01/incumbent.json \
  --candidate evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01/candidate.json \
  --incumbent-stages evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01/agent-managed-incumbent.json \
  --candidate-stages evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01/agent-managed-candidate.json \
  --output evals/paired/source-authority-local-ornith-http-status-diagnostic-2026-11-24/runs/run-01/agent-managed-comparison.json
```

Do not replay or alter the cohort after its first model-task request. Record
the status-class counts and frozen metric gates in `runs/run-01/report.md`.
