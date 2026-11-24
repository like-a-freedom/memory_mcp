# Invalid preflight attempt — missing temporary root

The Codex paired runner rejected the attempt because `/private/tmp/memory-mcp-source-authority-codex-2026-11-23` did not yet exist. It created the two empty receipt journals before checking that configuration. Both journals contain zero bytes; there were zero trials, model turns and tokens. No model call occurred. The valid capture uses a fresh `runs/run-01/` directory after precreating the registered temp root.
