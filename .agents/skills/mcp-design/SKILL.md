---
name: mcp-design
description: Design, review, and improve MCP servers and tools, including discoverability, schemas, responses, compatibility, and verification. Use when implementing or reviewing an MCP surface or diagnosing incorrect tool selection or use.
metadata:
  version: "2.4"
  author: "mcp-design-skill"
  tags: ["mcp", "ai-agents", "tool-design", "intent-driven", "conformance"]
---

# MCP server design

Design for the user's intent and the target clients' actual capabilities. Preserve existing public contracts unless a migration is in scope. Treat MCP requirements, optional extensions, host behavior, and local conventions as distinct; check the relevant protocol revision and implementation before making compatibility claims.

## Workflow

1. Identify the user goals, target clients, transports, permissions, and existing surface. Map representative requests to tools; combine deterministic steps when useful, and keep variable decisions available to the agent.
2. Design names, descriptions, schemas, results, and errors around those goals. Put argument constraints in schemas, selection boundaries in descriptions, and result-specific next steps in responses. Keep security and validation in the server even if a skill describes the workflow.
3. Check write safety, latency, and compatibility where relevant. Do not promise actions, data, recovery tools, or protocol features the implementation cannot provide.
4. Exercise representative requests through the actual transport and compare responses with authoritative state. For writes, use authorized isolated data. Report untested clients, transports, and unsupported capabilities.

## Load only the relevant guidance

- For tool names, arguments, response shapes, safe writes, or latency, read [tool design](references/tool-design.md).
- For dual-era server or client compatibility across MCP 2026-07-28 and earlier revisions, read [protocol compatibility](references/protocol-compatibility.md).
- For reusable workflows or the Skills extension, read [skills](references/skills.md).
- For long-running Tasks, read [tasks](references/tasks.md).
- For client input and approval, read [elicitation](references/elicitation.md).
- For file transfer, read [file transfer](references/file-transfer.md).
- For conformance checks, realistic scenarios, and regression cases, read [verification](references/verification.md).

Deliver only what the user requested. For a proposed interface, include the intent, tool contract, representative success and failure responses, and unresolved compatibility or implementation risks. For a review, prioritize actionable findings and evidence.
