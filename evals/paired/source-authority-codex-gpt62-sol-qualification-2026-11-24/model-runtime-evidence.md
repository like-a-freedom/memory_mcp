# Model/runtime evidence

OpenAI's official Codex model catalog lists GPT-6.2 Sol as available in Codex app and CLI and shows `codex -m gpt-6.2-sol` as the invocation. The official launch page likewise describes GPT-6.2 Sol as available in Codex and the API.

- [Codex models](https://developers.openai.com/codex/models)
- [GPT-6.2 Astra and Sol](https://developers.openai.com/codex/whats-new/gpt-62-astra-sol)

Local capability preflight: installed `codex-cli 0.171.0`; executable SHA-256 recorded in `fingerprints.json`; existing local login status succeeded without exposing identity; one ephemeral read-only smoke using only a synthetic fixed-output prompt selected `gpt-6.2-sol` and returned the expected response. The smoke is not a quality observation and is excluded from the qualification cohort.

The Codex CLI adapter cannot pin the immutable served model revision, so this registration records the requested alias and states the limitation.
