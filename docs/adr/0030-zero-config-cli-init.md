# ADR-0030: Add an Output-Only `init` Command for Zero-Config Host Setup

> Status: Accepted
> Date: 2026-08-06
> Related: ADR-0016 (public surface freeze), ADR-0028, ADR-0029
> Amends: ADR-0016 AD-2 and the frozen public-surface wording in `CONTEXT.md` and `docs/agent_integration/CONTRACT.md`.

## Context

A new user currently has to discover MCP host configuration, environment variables,
and database defaults before receiving a first recalled fact. Zero-config embedded
operation removes the database setup, but host registration still requires copy-paste
knowledge.

## Decision

Add one public CLI subcommand, `memory_mcp init`, with targets `vscode`,
`claude-desktop`, `codex`, `zed`, and `env`. The default target is `vscode`.
The command prints one deterministic, host-native snippet wrapped in a JSON result
object to stdout and never writes host files, changes environment variables, starts
a database, or performs network access.

This is an explicit exception to ADR-0016 AD-2: `init` is not a lifecycle verb,
does not expose a memory capability, and does not alter the eight-tool MCP surface.
The ordinary CLI surface therefore grows from the existing frozen list by exactly
one output-only onboarding command.

## Consequences

ADR-0016, `CONTEXT.md`, and `docs/agent_integration/CONTRACT.md` must say that the
ordinary CLI freeze is amended by this one exception. The command is safe to run
repeatedly and can be used in install documentation. Host configuration schemas
may evolve independently, so each target has a dedicated renderer fixture based
on the target's authoritative documentation and format.

## Non-goals

This command does not install the binary, download models, edit shell profiles,
configure remote credentials, or claim that Anno/ML startup is dependency-free.
