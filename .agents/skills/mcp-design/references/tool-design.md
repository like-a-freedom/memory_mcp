# Tool design

## Discoverability and names

- Map representative user requests to tool candidates. Keep independent capabilities separate when their permissions, ownership, or discovery needs differ; group a deterministic sequence when doing so reduces calls without hiding needed choices.
- Use specific verb–object names and a consistent vocabulary. Avoid synonyms that conceal meaningful differences. Check the MCP naming rules for the negotiated specification revision.
- Choose prefixes based on how target hosts expose and aggregate tools. Avoid redundant prefixes when the host namespaces them; retain disambiguating prefixes when hosts flatten names or users already depend on them. Measure tool selection instead of assuming one naming convention fits every client.
- A public rename can break clients, stored operation identities, audit records, and alerts. Inventory consumers and persisted identifiers; plan aliases or migration when needed. Keep idempotency identities stable across renames.

## Descriptions and schemas

- Explain what a tool does, when to choose it, meaningful boundaries, and available alternatives. Reference only tools that exist. Keep argument types, bounds, defaults, and enums in the schema; add cross-field rules where the agent can see them.
- Set and measure description and discovery budgets for the actual host. Eager and deferred loading have different costs; no universal character limit or tool count applies.
- Use simple schemas, including arrays or nested objects when relationships require them. Specify bounds and unknown-field behavior. Avoid changing accepted inputs without a compatibility plan.
- Align runtime validation with the schema. Document distinctions between similarly named fields and ensure any shared filter and result vocabulary is consistent.

## Results and errors

- Return the data needed for the next decision, with status, provenance, pagination, or guidance where useful. Follow the negotiated MCP result and error contract, particularly for task handles. Preserve existing response compatibility.
- Make guidance specific to the returned result and refer only to available actions. Explain partial results and continuation semantics. Do not advertise unpopulated fields; when historical data is legitimately absent, distinguish unknown from empty according to the documented contract.
- Give numeric results the denominator, time range, or freshness needed to interpret them. Do not add unrelated metadata to every result.

## Writes and latency

- When offering preview and apply, perform the same relevant validation and conflict checks. Revalidate on apply because state may change after preview; bind approvals to the proposed action where needed.
- For retryable writes with possible duplicate effects, use a stable idempotency key or equivalent guarantee. Document scope, retention, replay behavior, and conflicting reuse. Do not derive durable operation identity solely from a public tool name.
- Measure representative and worst realistic inputs against the target client's deadline. Inspect query plans when needed; choose indexes for actual predicates and workloads rather than prescribing one database strategy. Log timing and bounded, non-sensitive metadata.
