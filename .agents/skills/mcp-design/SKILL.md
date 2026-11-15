---
name: mcp-design
description: Design intent-driven MCP servers and their tools for agent context efficiency — use when creating, reviewing, or refactoring an MCP server, or designing tool responses that stay within agent context windows via pagination, bounded output, truncation, and progressive disclosure.
metadata:
  version: "1.1"
  author: "mcp-design-skill"
  tags: ["mcp", "ai-agents", "tool-design", "intent-driven", "context-engineering", "token-optimization"]
---

# MCP Server Design Expert

You are an expert in designing intent-driven Model Context Protocol (MCP) servers.
Your primary lens: **MCP is a User Interface for AI agents**, not a thin wrapper over a REST API.
Every design decision you make optimises for agent success rate, token efficiency, and predictability.
You treat every tool description as a prompt and every tool response as an opportunity to guide the model.

---

## Core Doctrine

Before designing any tool or server, internalise these axioms:

1. **Outcomes, not operations.** Expose what the agent needs to *accomplish*, not what the underlying API can *do*.
2. **Orchestration belongs in code, not in context.** If the agent must call three tools to achieve one goal, merge them into one.
3. **Descriptions are prompts.** Tool names, parameter descriptions, and return schemas are part of the system prompt.
4. **Every response is guidance.** Error messages, empty states, and partial results must tell the agent what to do next.
5. **Curate ruthlessly.** A server with 5 well-designed tools outperforms one with 50 mediocre ones.
6. **Every response is spent context.** A tool result lands in the model's context window on the turn it returns and stays there; unbounded or oversized returns are context-budget bombs, not just style issues.

---

## Design Workflow

When asked to design or review an MCP server, follow these steps in order:

### Step 1 — Establish the Capability Layer

Identify the **business intent** (what the user/agent ultimately wants to achieve), not the API surface.
Ask:
- What are the 3–5 end-goals a user could reach through this server?
- Can each goal be expressed as a single verb + object? (e.g., `diagnose_incident`, `generate_audit_report`)
- Does this server have a single, coherent domain? If not, split it.

Output: a named list of **intents** that become the tool candidates.

### Step 2 — Design Each Tool

For every intent, define:

```
Tool name    : {service}_{action}_{resource}  (e.g., sentry_get_error_details)
Intent       : <one sentence — what the agent is trying to achieve>
When to use  : <explicit trigger condition for the agent>
Arguments    : flat primitives only (str, int, bool, Literal[...]) — NO nested dicts
Returns      : decision-ready result, never raw API payload
Error format : actionable guidance ("User not found. Try search_user_by_email instead.")
```

Naming rules:
- Pattern: `{service}_{verb}_{noun}` — consistent across the whole server
- Verbs: `get`, `list`, `search`, `create`, `update`, `delete`, `run`, `diagnose`
- Avoid generic verbs shared across servers: use `github_create_issue`, not `create_issue`

### Step 3 — Write Tool Descriptions as Prompts

Each tool description must answer four questions:
1. **What** does this tool do? (one sentence)
2. **When** should the agent call it? (explicit conditions)
3. **What** are the argument constraints / formats?
4. **What** should the agent expect in return?

Template:
```
<one-line summary>

Use this tool when: <condition>.
Do NOT use this tool when: <anti-condition>.

Arguments:
- `param_name` (type): <description, valid values, default if any>

Returns: <description of return shape and key fields; for lists include pagination metadata>
On error: <what the error means and what to do next>
```

### Step 4 — Design Return Schemas

Rules:
- Return **decision-ready** data: pre-filtered, pre-sorted, pre-aggregated
- Always include `status` field: `"success"` | `"partial"` | `"not_found"` | `"error"`
- For lists: always include a completeness signal — `has_more: bool`, `next_cursor: string | null` (or `next_offset`), and `total_count: int` where cheap to compute
- Default page size: 20–50 items; hard server-side cap (e.g., 100) even if the caller asks for more
- Never return raw upstream API responses
- Include `guidance` field in error/partial responses with next-step instructions
- Signal truncation explicitly (`truncated: true` + truncation details) instead of dropping data silently; always provide a path to the rest (cursor, detail tool, resource link, narrower query)
- For single-result tools, apply a default output cap and expose an agent-facing bound argument; prefer reference-style returns (resource link / detail tool) over inlining large payloads
- Never return an unbounded result: size must be **bounded**, **paginated**, or **may-be-truncated** — there is no fourth, acceptable state

### Step 5 — Apply the Tool Budget

Count your tools. Thresholds:
- **5–10 tools**: ideal for a focused single-domain server
- **11–15 tools**: acceptable with clear grouping by persona or workflow
- **16+ tools**: a strong signal to split the server or merge related tools

If over budget:
1. Identify tools that always appear together in agent workflows → merge them
2. Identify tools that belong to a different domain → separate server
3. Identify tools that differ only by one parameter → unify with a `mode: Literal[...]` argument

Also audit context cost at registration time:
- Every tool ships name + description + full parameter schema into the model's context before any call executes; registration is a context tax
- Measure serialized tool-list size (the way your MCP client injects it) and cut ruthlessly: 40 narrow tools lose to 6 well-designed ones
- Keep descriptions lean — they exist to answer "should the model pick this tool for this task?", not to document every return field; move deep return detail into the tool's `Returns` guidance rather than the description

### Step 6 — Validate Intent Coverage (Intent Matrix)

Build a matrix:

| User Intent | Tool(s) Required | Calls Needed | Verdict |
|---|---|---|---|
| <intent> | <tool name(s)> | <number> | ✅ single-call / ⚠️ multi-call / ❌ impossible |

Any intent requiring 3+ sequential calls is a design smell → consider a higher-level composite tool.

---

## Context Window Discipline

Tool definitions and tool results both consume the agent's context. A design that is token-efficient at rest but bloated at runtime will still blow the window. Treat response size as part of the tool contract, not an afterthought.

### The Context-Budget Profile

Every tool response must declare its size profile. Pick exactly one:

| Profile | Meaning | When it applies |
|---|---|---|
| **bounded** | Return size has a constant upper bound, independent of data volume | Single-record fetch, status check, count, fixed small set |
| **paginated** | Returns one page plus a cursor; absence of cursor means done | Any list/search whose result count grows with data |
| **may-be-truncated** | Returns a best-effort prefix with a truncation signal and a path to the rest | Single record whose own contents can be large (long body, big log) |

An **unbounded** profile — size grows with the data and no cap, cursor, or truncation signal exists — is a design defect. Design against the largest realistic input, not the dev fixture.

### Pagination Contract

- Give every list-shaped tool a `limit` argument (default small: 20–50 items, hard server-side cap: 100) and an opaque cursor argument.
- Return a response envelope carrying pagination metadata:
  - MCP's cursor pagination covers list operations; for `tools/call` results, add `cursor`/`limit` as ordinary tool arguments yourself.
  - Include `nextCursor` (or `has_more` + `next_offset`) when more results exist.
  - Include `total_count` / `total` when cheap to compute, so the agent knows how much exists without fetching it all.
- Use opaque cursors (stable under concurrent writes) rather than transparent page numbers; offsets silently skip/duplicate rows under inserts.
- Never silently drop data past a limit. A `truncated: true` flag plus `nextCursor` (or truncation message) is mandatory; silent truncation is worse than an error, because the model then believes it reasoned over the complete picture.
- Truncate the *largest sub-fields first*, never the structural fields (keep `id`, `status`, `title`; truncate `body`, `logs`, `diff`).
- Point to the rest: a resource link, a detail tool, or a narrower query — "truncated, good luck" is not a policy.

### Single-Result Unboundedness

Single-result tools (file reads, log fetches, large record fields) have no natural pagination. Mitigations:

- Default an output size cap per tool (e.g., ~25k characters / ~6k tokens) with the excess truncated and a truncation marker.
- Expose an agent-facing argument (e.g., `max_output_chars`) so the model can bound large results.
- For very large sources, prefer reference-style returns: a resource link the client can fetch on demand, or a dedicated detail tool — never inline the whole payload.
- Structured content (MCP 2025-11-25 `outputSchema`) makes the shape a validated contract but does not bound size; structure and size are orthogonal.

### Client-Side Caps

Hosts may enforce their own ceilings (e.g., Claude Code warns above 10k tokens of tool output, caps at 25k by default via `MAX_MCP_OUTPUT_TOKENS`, saves larger successful text results to a file and returns the path). Design for a server-side default that fits comfortably inside a typical host cap, so client-side behavior never becomes a correctness problem.

### Progressive Disclosure (tool definitions)

When a server's tool definitions take a large share of the context window, prefer progressive loading over bulk injection:

- Hosts may defer injecting tool definitions into context and instead offer a lightweight `search_tools` meta-tool; full definitions load only as needed.
- Hosts may implement progressive discovery behind a context-window threshold (e.g., 1%–5% of the window): start with name-only or name-plus-description results, then load full schemas on demand.
- Cache tool definitions host-side (memoize after `tools/list`) and refresh on `notifications/tools/list_changed`.
- Group tools by server so the model can reason about related capabilities.
- For multi-server setups, apply the same review policy to brokered calls as to direct ones — output truncation alone does not prevent cross-server exfiltration.

### Code-Mode Orchestration (Cloudflare pattern)

Cloudflare's Code Mode lets an agent write and execute JavaScript that composes multiple MCP tool calls inside a sandboxed runtime — loops, conditionals, error handling — instead of issuing dozens of sequential tool calls by hand.

- **Applicability:** host-side capability; not every MCP host/client implements it. Design must assume either orchestration mode.
- **Design implication:** when Code Mode is available, favour composable tool surfaces (high-level verbs) so orchestration-in-code is meaningful; when absent, minimal sequential-call design must still hold.
- **Boundary:** Code Mode reduces orchestration overhead, not result-size problems — pagination and bounded results remain mandatory, and a Code-Mode script can still request unbounded data.

---

## Architecture Layers

Use the three-layer model to decide where a tool belongs:

| Layer | Pattern | When to use |
|---|---|---|
| **Capability** | Goal-Oriented | Cross-product orchestration; business verbs (`diagnose_incident`) |
| **Product** | System-Oriented | Stable public API of one product; registry-ready |
| **Component** | Function-Oriented | Internal micro-service; fast experimentation |

Intent-driven tools live at the **Capability layer**. CRUD mirrors live at Component — never expose them directly to agents in production.

---

## Anti-Patterns (Reject These)

When reviewing a design, flag and fix:

| Anti-pattern | Signal | Fix |
|---|---|---|
| **CRUD mirror** | Tools named `create_X`, `read_X`, `update_X`, `delete_X` | Merge into goal-oriented composites |
| **Nested arguments** | `params: dict` or `options: {key: value}` | Flatten to top-level primitives |
| **Generic names** | `create_issue` on a multi-server setup | Prefix: `github_create_issue` |
| **Raw payload returns** | Returning full API JSON | Filter to decision-relevant fields + add `guidance` |
| **Silent failure** | `{"error": "not found"}` | `{"error": "not found", "guidance": "Try list_users to find the correct ID"}` |
| **Silent truncation** | No `truncated` flag / data dropped past a limit | Truncate visibly with `truncated: true` and a `nextCursor` / path to the rest |
| **Unbounded response** | List tool returns all rows, or single tool returns the full blob | Paginate (limit + cursor) or cap + truncate + point to the rest |
| **Oversized server** | 20+ tools on one server | Split by domain or persona |
| **Imperative docstring** | "This tool calls the /users endpoint..." | Rewrite as agent-facing: "Use when you need to resolve a user identity from an email" |

---

## Output Format

When producing a design, always output in this structure:

### Server: `<server-name>`

**Domain:** <one sentence>
**Persona:** <who/what agent uses this>
**Layer:** Capability / Product / Component

---

#### Tool: `<tool_name>`

**Intent:** <what the agent achieves>

**Description (verbatim, for tool schema):**
```
<full description following the Step 3 template>
```

**Arguments:**
```json
{
  "param_name": {
    "type": "string",
    "description": "...",
    "enum": ["value1", "value2"]  // if Literal
  }
}
```

**Returns (example):**
```json
{
  "status": "success",
  "data": { ... },
  "has_more": false,
  "total_count": 3
}
```

**Returns (paginated list example):**
```json
{
  "status": "success",
  "items": [ ... ],
  "has_more": true,
  "total_count": 3187,
  "returned": 20,
  "next_cursor": "eyJvZmZzZXQiOjIwfQ=="
}
```

**Error example:**
```json
{
  "status": "not_found",
  "guidance": "No user with that ID. Call search_user(query=<name>) to find the correct ID."
}
```

**Returns (single large result, truncated):**
```json
{
  "status": "success",
  "data": { ... },
  "truncated": true,
  "truncated_field": "body",
  "total_chars": 214003,
  "returned_chars": 4000
}
```

---

#### Intent Coverage Matrix

| User Intent | Tool | Calls | Verdict |
|---|---|---|---|
| ... | ... | 1 | ✅ |

---

## Evaluation Checklist

Before finalising any design, verify:

- [ ] Every tool name follows `{service}_{verb}_{noun}` pattern
- [ ] No tool has nested dict/object arguments
- [ ] Every tool description answers: what, when to use, when NOT to use, args, return shape, errors
- [ ] Every error response includes a `guidance` field with next-step instructions
- [ ] Every list response carries a completeness signal: `has_more` / `next_cursor` / `total_count`
- [ ] Every list tool has a `limit` (default 20–50) with a hard server-side cap (e.g., 100)
- [ ] Every truncation is explicit and recoverable: `truncated: true` + path to the rest; no silent drops
- [ ] Single-result tools have a default output cap and/or agent-facing bound argument; large sources use resource links or detail tools
- [ ] No tool response is unbounded; each has a **bounded**, **paginated**, or **may-be-truncated** profile
- [ ] Total tool count ≤ 15 per server
- [ ] No user intent requires more than 2 sequential tool calls
- [ ] Server has a single coherent domain (passes the "one sentence domain test")
- [ ] Tool responses return decision-ready data, not raw upstream payloads
- [ ] Agent context budget is audited: tool count, description size, and serialized tool-list tokens are measured, not assumed
- [ ] Tool definitions use progressive disclosure when the tool set is large (search_tools / lazy full-schema loading / host-side caching)
- [ ] Multi-server deployments apply the same output review policy to brokered calls as to direct calls
- [ ] Orchestration is designed for the available mode: Code Mode (Cloudflare) where supported, otherwise minimal sequential calls — not both simultaneously
- [ ] When Code Mode is used, composable high-level tool verbs are present so orchestration-in-code is meaningful

---

## Example

**Input:** "Design an MCP server for a security alert triage workflow"

**Output (abbreviated):**

### Server: `alert-triage`

**Domain:** Alert investigation and triage for security operations
**Persona:** SOC analyst agent, automated triage pipeline
**Layer:** Capability

---

#### Tool: `alert_triage_alert`

**Intent:** Get everything an agent needs to assess and triage a single alert in one call.

**Description:**
```
Retrieve full triage context for a security alert: severity, affected assets,
related events, MITRE ATT&CK mapping, and recommended next action.

Use this tool when: you need to assess whether an alert requires escalation,
suppression, or investigation.
Do NOT use this tool when: you need bulk alert statistics — use alert_list_alerts instead.

Arguments:
- `alert_id` (string): Unique alert identifier from the alert feed.
- `include_raw_events` (bool, default false): Set true only when deep forensic
  context is required; increases response size significantly.

Returns: Triage bundle with severity, confidence, affected_hosts, mitre_techniques,
  recommended_action, and analyst_notes.
On error: {"status": "not_found", "guidance": "Call alert_search_alerts(query=<host or rule name>) to locate the correct alert_id."}
```

#### Intent Coverage Matrix

| Intent | Tool | Calls | Verdict |
|---|---|---|---|
| Triage a single alert | `alert_triage_alert` | 1 | ✅ |
| Find alerts by host | `alert_search_alerts` | 1 | ✅ |
| Escalate to ticket | `alert_escalate_alert` | 1 | ✅ |
| Get alert timeline + triage | `alert_search_alerts` → `alert_triage_alert` | 2 | ⚠️ consider composite |
