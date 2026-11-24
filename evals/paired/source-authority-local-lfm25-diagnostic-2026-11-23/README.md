# LFM2.5 failure-class diagnostic

This frozen, one-capture diagnostic asks whether a fresh, disjoint four-family
synthetic source-authority cohort can explain the LFM2.5 pilot failures after
the provider-neutral adapter began distinguishing invalid response envelopes,
final-answer schemas and tool-call schemas.

It is not an acceptance benchmark. Both arms use the same product binary, so
there is no memory treatment effect. Four families cannot establish broad
reliability or agent utility. A pass would be a feasibility observation only.
No retries, task edits, prompt tuning or repeat capture are allowed on these
families.

The model interface is the generic OpenAI-compatible Chat Completions adapter.
LM Studio is the local provider used here; neither the deterministic tests nor
the runner protocol require it. The tasks contain synthetic facts only and the
runner is configured for numeric loopback. The model-weight and server-build
hashes are unavailable and recorded as such.

## Frozen-input verification

From the repository root, verify the frozen inputs with shasum -a 256 -c evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/freeze-manifest.sha256 on macOS/Linux.
PowerShell users can compare each entry with Get-FileHash -Algorithm SHA256.

## Registered local capture

Run once, after verifying the manifest and confirming the exact local model alias
is loaded. Do not reuse the output directory.

~~~sh
cargo build --locked -p memory_mcp
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions   --plan evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/plan.json   --tasks evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/tasks.json   --model-endpoint http://127.0.0.1:1234/v1   --incumbent-memory-server "$PWD/target/debug/memory_mcp"   --candidate-memory-server "$PWD/target/debug/memory_mcp"   --output-directory evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/runs/run-01
~~~

On Windows, use the same command with PowerShell continuation and
target/debug/memory_mcp.exe. For another compatible local provider, prepare a
new plan with its exact model identity and endpoint fingerprint; never edit this
frozen registration after running it.
