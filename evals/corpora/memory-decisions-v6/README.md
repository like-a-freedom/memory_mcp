# Memory decisions v6 rollout replication

This corpus evaluates the claim-reconciliation rollout on eight fixed public
MCP scenarios. It uses event, source, subject, value, and date strings that do
not occur in v5. It deliberately retains the same eight scenario templates as
v5 (absence, similar names, singleton, peer conflict, unverified correction,
prior-knowledge cutoff, valid-time boundary, and ordinary location). It is a
disjoint-data replication, not a new task-family study or a population sample.

The default and explicit `shadow` profiles use the same product binary and
corpus. The acceptance plan compares the normalized **primary** public
observation, requires identical repeated captures, no per-case evidence or
ordinary-decision regression, zero newly introduced safety failures, a
minimum 25 percentage-point equal-family decision gain, and at most five
returned items. A safety failure already present under `shadow` is reported as
an incumbent limitation; it is not counted as a candidate regression.

The profile requires no agent, model provider, or API key. Its score describes
observable memory evidence and decision signaling, not downstream agent task
success or usability.
