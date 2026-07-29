# Web Search & Fetch via Keenable CLI

Keenable is used as a **CLI binary**, not as an MCP server. All interaction goes through the terminal.

## Installation

```bash
brew install keenableai/tap/keenable-cli
```

Or from source:
```bash
cargo install --git https://github.com/keenableai/keenable-cli
```

## Search

```bash
# YAML output (for agents)
keenable search "rust async patterns"

# Pretty output (for humans)
keenable search "rust async patterns" -p

# Restrict to one site
keenable search "anthropic" --site techcrunch.com

# Date filter
keenable search "AI news" --published-after 2026-05-01
```

## Fetch a page as clean markdown

```bash
keenable fetch https://example.com
```

## Authentication (optional, raises rate limits)

```bash
keenable login          # Device-code flow — works in headless/agent environments
keenable login --api-key keen_***
```
