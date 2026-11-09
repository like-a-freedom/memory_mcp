# Ingestion source authority and secret-safe persistence

- **Date:** 2026-11-09
- **Status:** Proposed design; no implementation authorized by this document
- **Baseline:** `3622ceb3` (`v1.38.0`)
- **Scope:** Existing `ingest` capability in local and Streamable HTTP profiles; no new MCP tool

## Intent and evidence

An `ingest` caller submits `content`, but the server currently decides whether that string is inline text, an existing file, an existing directory, or an HTTP(S) URL. The decision can initiate file or network I/O before the prepared request is validated. The same service is used under different trust assumptions. Define source authority at the composition boundary, so the server never infers permission to read local files or fetch URLs merely from untrusted content.

This is a trust-boundary design, not a claim that an SSRF exploit has been demonstrated. The URL fetcher already validates public addresses, pins the resolved address, disables proxies and redirects, and limits time and response size. Those protections remain useful when URL import is explicitly authorized. Connection strings that are not HTTP(S) currently fall through as inline text; they are a possible secret-persistence problem, not evidence of an attempted database connection.

| Evidence | Consequence |
| --- | --- |
| [`detect_ingest_transport` and `prepare_ingest_request`](../../../crates/memory-mcp/src/memory/content_extraction.rs) check `Path::exists()` and may read a file/directory, then consider HTTP(S). | Source selection and authority need separate decisions. |
| [`IngestionService::ingest_with_metadata`](../../../crates/memory-mcp/src/memory/ingestion.rs) calls preparation before `validate_ingest_request` and logs the caller's `source_id` by default. | Validate and redact before I/O or logs; do not put raw identifiers in errors. |
| [`CapturePolicy`](../../../crates/memory-mcp/src/memory/agent_memory/policy.rs) already has a secret-like-content rule for lifecycle capture. | Reuse its intent and tests where applicable, but verify its false-positive behavior before applying it to generic ingest. |
| [OWASP SSRF guidance](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html) distinguishes trusted-destination and arbitrary-external cases, recommends constrained protocols, validated destinations and redirect control. | Preserve the existing network controls even behind an authority gate. |
| [cognee v1.6.7](https://github.com/topoteretes/cognee/releases/tag/v1.6.7) rejects raw connection strings and unsafe paths by default. | Useful prior art, not a drop-in policy for this project's local and SaaS profiles. |

## Goals, non-goals, constraints

**Goals**

1. In the HTTP SaaS profile, an untrusted `content` value cannot cause server-side file, directory, or URL reads without a separate, explicit, authorized import path.
2. Generic ingestion cannot persist recognized credentials or echo them in logs and errors. The policy reports an actionable refusal reason without returning the secret.
3. Local single-user ingestion keeps a deliberate route for importing local files and public URLs; existing legitimate use cases have a documented migration path.
4. Maintain deterministic episode identity, provenance, bi-temporal timestamps, tenant isolation, and the existing eight-tool surface.

**Non-goals:** DLP for arbitrary secrets, prompt-injection classification, malware scanning, a new connector framework, a new MCP tool, or physical deletion of previously stored content. An inferred secret is not silently rewritten inside an episode or extracted fact; silent redaction would change evidence.

**Constraints:** Use a context-local ingestion use case with narrow policy input. Keep transport handlers thin. Do not change dependencies, migrations, generated code, or the public tool count as a by-product of this design. If a public request schema change or a new tool becomes necessary, stop for the repository's ADR/approval gate.

## Chosen design

### 1. Separate representation from authority

The composition root supplies an immutable `IngestSourcePolicy` (name illustrative), not a caller-controlled flag. Its decision distinguishes `inline`, `local_file`, `local_directory`, and `remote_url` and is evaluated **before** any source I/O. The service must not equate a syntactically recognizable path or URL with permission to import it.

| Profile / entry point | Default meaning of `content` | Authorized import route |
| --- | --- | --- |
| Streamable HTTP MCP and control-plane clients | Literal inline content only. Existing path strings and URL strings stay literal unless rejected by secret policy. | A future import operation requires explicit authority and its own review; this spec does not add one. |
| Local stdio MCP and `memory_mcp ingest` CLI | Preserve the current path/URL convenience for the trusted single-user profile during migration, but make it a documented profile policy rather than an emergent parser rule. | Local profile policy explicitly enables path/URL import. A later compatibility change may move this behind explicit CLI intent after usage is measured. |
| Filesystem watcher | Its configured inbox only. | Keep the watcher containment and symlink rules; do not route it through generic unrestricted path detection. |

The exact internal type and constructor are builder choices. The observable contract is fixed: an HTTP caller cannot change source authority via `source_type`, `source_id`, `policy_tags`, content spelling, or MCP access fields. The local policy is selected at startup. Existing URL validation remains a second line of defense when local URL import is enabled. The builder must trace every HTTP and local construction path to prove the intended policy reaches the ingestion use case.

### 2. Validate before side effects and persistence

Order the work as: rate/access check → cheap request-shape and submitted-string secret validation → source policy classification → safe diagnostic identifier → optional authorized source I/O → post-extraction content validation → durable write and extraction. For HTTP inline content, classification must be side-effect-free, including no filesystem existence probe. Validate the final extracted text because a safe-looking path may point to credentials. A credential-bearing path, URL userinfo, or URL query token is refused **before** a local read, DNS lookup, or network request. Authenticated URL import through credentials embedded in the URL is not part of this contract.

Recognize concrete credential-bearing formats (for example a URI with userinfo/password or known token syntax) in all persistence-bound text and identifying metadata, including `source_id`. Refuse on a positive match with a typed validation reason in every profile, including local CLI, stdio, HTTP, and watcher ingestion; there is no trusted override in this design. Do not promise perfect detection, and do not globally reject ordinary documentation about connection strings. Avoid returning raw `content`, URL query strings, path names, or `source_id` in errors/log fields. Any diagnostic correlation uses a bounded non-reversible identifier and reason code. Review the existing lifecycle `resembles_secret` rule for reuse rather than creating an inconsistent second heuristic.

For an authorized URL import, keep scheme restriction, DNS validation/address pinning, redirect and proxy disabling, limits, and response-size enforcement. Reject unsupported URL schemes as import targets. For local file/directory import, retain the watcher-specific containment independently; do not assume generic local import has watcher containment.

### 3. Rollout and compatibility

Make the HTTP policy fail closed at the shared ingestion seam, not only in one handler. Add explicit local/HTTP contract documentation and deprecation notes if local auto-detection is later removed. Existing stored episodes are not rewritten. If a suspected credential is already stored, incident response and deletion policy are separate work.

## Alternatives considered

| Approach | Decision | Reason |
| --- | --- | --- |
| Only add a connection-string blacklist | Reject | Paths and URLs remain ambient authority; blacklist evasion and false positives remain. |
| Disable all URL/path import in every profile immediately | Reject | Breaks the documented local workflow without requiring it for the HTTP boundary. |
| Add a public `transport` parameter or ninth MCP tool | Defer | Requires a schema/ADR decision and cannot be inferred from this security design alone. |
| Redact matched substrings in raw memory | Reject | Produces altered evidence and may leave derived copies inconsistent. Refusal before write is auditable. |

## Acceptance evidence

1. Integration tests exercise the real HTTP and local composition paths. HTTP requests containing an existing server path, directory path, public URL, localhost URL, `file://` string, and common connection-string examples cause **zero** filesystem/network import calls. Ordinary literal discussion of a URL or connection string remains possible if it contains no credential.
2. Local authorized file and public-URL imports still work; the URL tests cover redirects, DNS rebinding/pinning, non-public addresses, oversized responses, and timeout behavior without relaxing existing protections.
3. A controlled adapter captures file/network calls and log/error output. Credential-bearing input is refused before durable state or extraction; tests assert absence of the credential from logs, errors, and stored rows. Include positive and negative detector cases.
4. The final content after authorized extraction receives the same secret check; a file containing a credential is refused without a partial episode. A failed import cannot leave a partially written episode or derived facts.
5. Cross-tenant and local-profile tests show that `source_type`, `source_id`, and `policy_tags` cannot grant extra authority. Document the operational boundary and known detector limitations.

## Delivery boundary

This design can be implemented independently of the memory-quality and LongMemEval-V2 specifications. Before implementation, confirm every active entry point's policy wiring. Before shipping, follow the repository's targeted tests and workspace clippy gate. The present document performs no implementation or release action.
