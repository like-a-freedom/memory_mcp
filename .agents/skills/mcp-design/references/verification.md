# Verification

Use checks that cover the change and its failure modes. A descriptor snapshot detects surface drift but cannot establish correct results or user intent coverage.

## Contract checks

- Compare advertised input types, bounds, defaults, and output schemas with actual validation and responses. Check that referenced tools and recovery paths exist.
- Check names and descriptions against each target host's discovery and namespace behavior. Measure selection success, context cost, and call count on representative requests.
- If the project uses descriptor snapshots, inspect the diff when updating them. Test compatibility and migration for intentional changes; snapshots alone do not determine whether a change is safe.

## End-to-end scenarios

- Run changed workflows over the actual transport with representative clients and controlled data. Compare results against authoritative state and exercise missing, partial, invalid, and boundary inputs.
- For writes, compare preview and apply on conflicts; test duplicate submissions and any idempotency guarantee. Use disposable data and appropriate authorization.
- For latency, include the widest realistic input and measure it against the relevant deadline. A fast narrow query does not prove the broad one meets the limit.
- For servers supporting modern and legacy revisions, follow the transport and protocol scenarios in [protocol compatibility](protocol-compatibility.md). For extensions, test negotiated versions, capability mismatches, unsupported clients, and the lifecycle and failure paths in the relevant reference linked from [SKILL.md](../SKILL.md). A typed SDK object or a successful synchronous call does not establish task wire compatibility.
- Record the clients, protocol versions, transports, and cases actually exercised; report missing access or fixtures instead of claiming unverified coverage.
