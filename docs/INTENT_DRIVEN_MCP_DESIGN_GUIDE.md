# Curated Sources on Intent- and Skills-Driven MCP Design

## Overview

By late 2025 and early 2026, designing MCP servers around user intent rather than API endpoints had matured into a recognizable discipline of its own. This guide gathers the most credible and up-to-date materials I found, organized by category: from official Anthropic engineering guidance to enterprise design patterns, academic papers, and community discussions.

***

## Official Anthropic guidance

### Writing Effective Tools for AI Agents (September 2025)

Anthropic’s flagship engineering article on the full workflow: prototype → evaluate → iterate with agents. Its core principles are:[^1]

- **Outcomes, not operations** — instead of exposing `list_users` + `list_events` + `create_event`, expose something like `schedule_event` that finds availability and books the meeting for the agent.[^1]
- **Namespacing** — prefer names like `asana_projects_search` and `jira_search` over a generic `search`. Anthropic notes that prefix-based versus suffix-based namespacing can materially affect evaluation results.[^1]
- **Meaningful context** — return fields such as `name`, `image_url`, and `file_type` rather than `uuid`, `256px_image_url`, or `mime_type`. Replacing opaque identifiers with human-readable ones significantly reduces hallucinations.[^1]
- **Response format enums** — a `concise` versus `detailed` response mode, similar in spirit to GraphQL, lets the agent control how much context it receives.[^1]
- **Prompt-engineered descriptions** — tool descriptions are effectively prompts. Even small wording changes can produce dramatic improvements in evaluation quality.[^1]

📎 https://www.anthropic.com/engineering/writing-tools-for-agents

### Code Execution with MCP (November 2025)

Anthropic’s engineering post on the pattern of **code execution instead of direct tool calls** for scale. Key ideas include:[^2]

- **Progressive disclosure** — tools are presented as files in a filesystem, so the agent loads definitions on demand rather than consuming 150K tokens up front.[^2]
- **Context-efficient results** — the agent writes code that filters 10K lines inside the execution environment and only returns the five lines it actually needs.[^2]
- **Privacy-preserving flows** — intermediate sensitive data can remain inside the execution environment instead of entering the model’s context window.[^2]
- **Skills as persisted code** — agents can save working code as reusable functions plus `SKILL.md`, creating a bridge between MCP tools and Anthropic Agent Skills.[^2]

📎 https://www.anthropic.com/engineering/code-execution-with-mcp

***

## Practical industry guides

### Phil Schmid — MCP Best Practices (January 2026)

One of the most widely cited practical guides. It distills six rules:[^3]

| # | Rule | Summary |
|---|------|---------|
| 1 | Outcomes, Not Operations | One tool like `track_latest_order(email)` instead of three separate calls |
| 2 | Flatten Arguments | Only top-level primitives and `Literal`s; no nested dictionaries |
| 3 | Instructions are Context | Docstrings act as prompts; errors should provide guidance |
| 4 | Curate Ruthlessly | Aim for 5–15 tools per server; one server, one job |
| 5 | Name for Discovery | Follow a `{service}_{action}_{resource}` pattern |
| 6 | Paginate Large Results | Always include `has_more`, `next_offset`, and `total_count` |

📎 https://www.philschmid.de/mcp-best-practices

### Workato — 9 Patterns for Composable Skills-Based Tool Design (December 2025)

The most detailed enterprise case study in this set, complete with before-and-after benchmarks. Using a hotel management system example, it shows the shift from 11 API wrappers to 2 intent-driven tools.[^4]

**The nine patterns:**

1. **Accept Business Identifiers, Not System IDs** — use `guest_email` instead of `contact_id: "003Dn00000QX9fKIAT"`.[^4]
2. **Build Idempotency Into Tool Design** — require an `idempotency_token` for every mutating operation.[^4]
3. **Coordinate State Transitions Atomically** — use one `check_in_guest` tool instead of three separate status updates.[^4]
4. **Embed Authorization in Tool Design** — prefer `search_cases_on_behalf_of_guest(email)` over `search_all_cases`.[^4]
5. **Provide Smart Defaults** — the agent should only need to supply genuinely variable parameters.[^4]
6. **Document Prerequisites and Error Modes** — explain likely failure modes before the call, not after.[^4]
7. **Support Partial Updates** — only specified fields change; everything else stays intact.[^4]
8. **Create Defensive Composition Helpers** — helpers such as `create_contact_if_not_found(email, ...)` make prerequisite orchestration safe and idempotent.[^4]
9. **Design for Natural Language Patterns** — tool names should align with how users naturally speak.[^4]

**Benchmarks (Dewy Resort):**

| Metric | Before | After |
|---------|--------|-------|
| Response time | 8–12 sec | 2–4 sec |
| Success rate | 73% | 94% |
| Number of tools | 47 | 12 |
| Tool calls per interaction | 6.2 | 1.8 |

📎 https://www.workato.com/the-connector/designing-skills-for-enterprise-mcp/

### The AI Stack — 5-Step API-to-MCP Conversion (August 2025)

A step-by-step process for converting a REST API into MCP tools:[^5]

1. **Consolidate by intent** — use `manageUserProfile(action: create|update|delete)` instead of three CRUD endpoints.
2. **Agent-guiding responses** — every response includes `next_actions` and `suggestion` for the agent.
3. **Optimize tokens** — use batching, field filters, and keep the total tool count around 15–20.
4. **Permissions and risk bands** — separate safe from risky operations and require `confirm_token` for destructive actions.
5. **Test with MCP Inspector** — track metrics such as `tool_selection_frequency`, `error_rate_by_tool`, and `avg_response_time`.

📎 https://www.theaistack.dev/p/your-api-to-mcp-conversion-is-broken

### Docker — Top 5 MCP Server Best Practices (July 2025)

Docker’s guide approaches MCP from a packaging and deployment angle, but it makes one architectural point especially well:[^6]

- **Tool Budget** — consciously manage how many tools the server exposes; too many tools can make a server harder to adopt.[^6]
- **The end user is the agent** — error messages should help the agent recover, not just explain the failure to a human.[^6]
- **Self-contained tool calls** — each tool call should create its own connection, trading some latency for reliability and usability.[^6]
- **Document for two audiences** — humans decide whether to enable the server, while agents rely on names and descriptions to select tools.[^6]

📎 https://www.docker.com/blog/mcp-server-best-practices/

***

## Enterprise architecture and patterns

### IBM — Think in Intents, Not APIs (June 2025)

This article describes a three-layer enterprise architecture for MCP servers:[^7]

| Layer | Pattern | Example |
|-------|---------|---------|
| **Capability-Level** | Goal-Oriented | `diagnoseIncident`, `generateAuditReport` — cross-product orchestration |
| **Product-Level** | System-Oriented | A stable wrapper around one product’s APIs, such as Instana or Turbonomic |
| **Component-Level** | Function-Oriented | Individual microservices for internal use |

The key point is that the **Capability layer** is where the truly intent-driven pattern lives: tools are named after business goals, not APIs.[^7]

📎 https://gyliu513.github.io/jekyll/update/2025/06/10/mcp-pattern.html

### CodeNinja — Enterprise MCP Deployment Patterns (June 2025)

An architectural analysis of intent-driven enterprise systems, framing deployment around intent → policy → execution, with compliance and auditability built in.[^8]

📎 https://codeninjaconsulting.com/blog/intent-driven-systems-analysis-model-context-protocol-mcp-enterprise-enviornments

### Agentico — Intent-Based Server for MCP (April 2025)

An open-source experiment that applies Kubernetes-style ideas — manifests, reconciliation, and declarative infrastructure — to MCP server management. Toolsets are defined in `server.yaml`, and tools are loaded dynamically from the desired state.[^9]

📎 https://www.reddit.com/r/mcp/comments/1jzf97f/intentbased_server_for_mcp_servers_alpha_release/

***

## Skills vs. tools — an architectural distinction

### Arcade.dev — Skills vs Tools for AI Agents (December 2025)

A strong analysis of how tools and skills differ architecturally:[^10]

- **Tools** are the agent’s hands: they execute actions across APIs, databases, and files.
- **Skills** are the agent’s training: reusable instructions, context, domain knowledge, and behavioral patterns.
- **Token economics matter** — one GitHub MCP server can expose 90+ tools and 50K+ tokens of schema, while skills capture expertise without the same schema overhead.
- **Key takeaway** — from the model’s perspective, both look similar: a description plus an invocation path. The distinction matters more to system designers than to the agent itself.

📎 https://www.arcade.dev/blog/what-are-agent-skills-and-tools/

### Claude Skills vs MCP (December 2025 – February 2026)

These sources frame Skills as filesystem-based modules with YAML frontmatter and Markdown instructions, while MCP remains a protocol for external tool interfaces. The most effective architecture usually combines them: MCP fetches data, Skills interpret and process it.[^11][^12]

📎 https://dev.to/jimquote/claude-skills-vs-mcp-complete-guide-to-token-efficient-ai-agent-architecture-4mkf
📎 https://intuitionlabs.ai/articles/claude-skills-vs-mcp

***

## A central pattern: tool responses as prompts

### Reddit r/mcp — Every Tool Response is an Opportunity to Prompt the Model (July 2025)

This community post captures a core idea cleanly: tool responses are not just data; they are prompts for the model.[^13][^14]

- The agent has no durable memory of an API workflow, so each response should suggest the next best step.
- A successful response should combine data with guidance, for example: `"Found 42 active members. Use bulk() to contact them."`
- Errors should redirect, not merely fail.
- If a tool works more than 90% of the time, do not overload the description with edge cases; use error responses to correct course instead.

📎 https://www.reddit.com/r/mcp/comments/1lq69b3/good_mcp_design_is_understanding_that_every_tool/

***

## Security and intent validation

### Microsoft GenAIScript — MCP Intent Validation (April 2025)

This post presents an **LLM-as-a-Judge** pattern for validating whether a tool response actually matches the tool’s intended purpose. If a `weather` tool starts returning the contents of `package.json`, the validator blocks it before the result reaches the model context.[^15]

📎 https://microsoft.github.io/genaiscript/blog/mcp-intents/

### MCP Security Bench / Prompt Injection Attacks (2025–2026)

OpenReview papers show that natural-language tool metadata itself becomes a prompt-injection surface. Attacks such as name collisions, preference manipulation, and tool transfer highlight how important careful naming, namespacing, and description design really are.[^16][^17]

📎 https://openreview.net/pdf?id=xKDHZLQJ2O  
📎 https://openreview.net/pdf?id=91LcHUG0OM

***

## Network domain example: intent-based network management via MCP

### IETF Draft — MCP for Intent-Based Network Troubleshooting (October 2025)

This IETF draft maps MCP roles to the network management domain:[^18]

- network devices act as MCP servers and expose Resources, Tools, and Prompts
- network controllers act as MCP clients and host the LLM in a sandbox without direct L3 access to devices
- the interaction model is intent-first, for example: “verify reachability between Site-A and Site-B” instead of issuing CLI commands
- all interactions use JSON-RPC 2.0 with an auditable trail, such as syslog

It is particularly relevant to security and XDR thinking because the same pattern can be adapted to intent-based threat investigation.

📎 https://www.ietf.org/archive/id/draft-zeng-mcp-troubleshooting-00.html

***

## Academic work and benchmarks

### Context-Aware MCP with a Shared Context Store (January 2026)

This paper extends MCP so that specialized servers can read and write to a shared context store, coordinating more autonomously without repeated prompting. On the TravelPlanner benchmark, CA-MCP reduces both LLM call volume and failure frequency.[^19]

📎 https://arxiv.org/abs/2601.11595

### Agent Context Protocols (ACPs) — OpenReview

ACP complements MCP with persistent execution blueprints, standardized `AGENT_REQUEST` and `AGENT_RESPONSE` schemas, and explicit error codes for fault tolerance. On AssistantBench, it reports state-of-the-art results.[^20]

📎 https://openreview.net/pdf?id=xKDHZLQJ2O

### Efficient On-Device Agents via Adaptive Context (OpenReview)

This work focuses on edge deployment: minimalist tool schema serialization plus just-in-time loading of full definitions, producing a reported 6× context reduction on a 3B SLM.[^21]

📎 https://openreview.net/pdf/79d13e5b1f25fe7172b32745ea465ff5abe702e7.pdf

***

## Source summary table

| Source | Author / Organization | Date | Focus | Credibility |
|----------|---------------------|------|-------|-------------|
| Writing Effective Tools for AI Agents | Anthropic Engineering | Sep 2025 | Evaluation-driven tool design | ★★★★★ |
| Code Execution with MCP | Anthropic Engineering | Nov 2025 | Progressive disclosure, Skills | ★★★★★ |
| MCP Best Practices | Phil Schmid | Jan 2026 | Six practical rules | ★★★★☆ |
| 9 Patterns for Composable MCP | Workato (Zayne Turner) | Dec 2025 | Enterprise skills-based design | ★★★★☆ |
| Think in Intents, Not APIs | IBM (Guangya Liu, Ruchi Mahindru) | Jun 2025 | Three-layer architecture | ★★★★☆ |
| Skills vs Tools for AI Agents | Arcade.dev | Dec 2025 | Architectural differentiation | ★★★★☆ |
| Docker MCP Best Practices | Docker | Jul 2025 | Packaging and tool budget | ★★★★☆ |
| 5-Step API-to-MCP Conversion | The AI Stack | Aug 2025 | Step-by-step conversion guide | ★★★☆☆ |
| Tool Response as Prompt | Reddit r/mcp | Jul 2025 | Community design pattern | ★★★☆☆ |
| IETF MCP Network Troubleshooting | IETF Draft | Oct 2025 | Network and security domain | ★★★★☆ |
| MCP Intent Validation | Microsoft GenAIScript | Apr 2025 | Security pattern | ★★★☆☆ |
| Context-Aware MCP (CA-MCP) | arXiv | Jan 2026 | Shared context store | ★★★☆☆ |
| Agentico Intent-Based Server | La Rebelion Labs | Apr 2025 | Kubernetes-like MCP orchestration | ★★☆☆☆ |

---

## References

1. [Writing effective tools for AI agents—using AI agents - Anthropic](https://www.anthropic.com/engineering/writing-tools-for-agents) - The Model Context Protocol (MCP) can empower LLM agents with potentially hundreds of tools to solve ...

2. [Code execution with MCP: building more efficient AI agents - Anthropic](https://www.anthropic.com/engineering/code-execution-with-mcp) - Learn how code execution with the Model Context Protocol enables agents to handle more tools while u...

3. [MCP is Not the Problem, It's your Server: Best Practices for Building ...](https://www.philschmid.de/mcp-best-practices) - Developers treat MCP like a REST API wrapper. MCP is a User Interface for Agents. Different users, d...

4. [Designing Composable Tools for Enterprise MCP - Workato](https://www.workato.com/the-connector/designing-skills-for-enterprise-mcp/) - 9 patterns for designing composable tools in enterprise MCP: idempotency, business identifiers, atom...

5. [API to MCP Tools: 5 Step Process to create one](https://www.theaistack.dev/p/your-api-to-mcp-conversion-is-broken) - Uncover the 5-step agent-first pattern to transform fragmented API endpoints into powerful, task-ori...

6. [Don't just test functionality, test...](https://www.docker.com/blog/mcp-server-best-practices/) - Design secure, scalable MCP servers using these 5 best practices. Learn how to test, package, and op...

7. [Think in Intents, Not APIs: MCP Architecture Patterns for AI ...](https://gyliu513.github.io/jekyll/update/2025/06/10/mcp-pattern.html) - Table of Contents generated with DocToc

8. [Enterprise MCP Deployment Patterns Analysis to Best ... - CodeNinja](https://codeninjaconsulting.com/blog/intent-driven-systems-analysis-model-context-protocol-mcp-enterprise-enviornments) - Discover how MCP enables intent-driven automation by replacing traditional interfaces with AI orches...

9. [Intent-based server for MCP Servers (Alpha release, like Kubernetes but for AI)](https://www.reddit.com/r/mcp/comments/1jzf97f/intentbased_server_for_mcp_servers_alpha_release/) - Intent-based server for MCP Servers (Alpha release, like Kubernetes but for AI)

10. [Skills vs Tools for AI Agents: Production Guide - Arcade.dev](https://www.arcade.dev/blog/what-are-agent-skills-and-tools/) - Tools execute actions. Skills provide expertise. Learn the architectural difference that determines ...

11. [Claude Skills vs MCP: Complete Guide to Token-Efficient ...](https://dev.to/jimquote/claude-skills-vs-mcp-complete-guide-to-token-efficient-ai-agent-architecture-4mkf) - Learn when to use Claude Skills, MCP (Model Context Protocol), or both — with real-world examples an...

12. [Claude Skills vs. MCP: A Technical Comparison for AI Workflows](https://intuitionlabs.ai/articles/claude-skills-vs-mcp) - Learn the key differences between Anthropic's Claude Skills and the Model Context Protocol (MCP).

13. [Good MCP design is understanding that every tool response is an opportunity to prompt the model](https://www.reddit.com/r/mcp/comments/1lq69b3/good_mcp_design_is_understanding_that_every_tool/) - Good MCP design is understanding that every tool response is an opportunity to prompt the model

14. [Good MCP design is understanding that every tool response is an ...](https://www.reddit.com/r/LLMDevs/comments/1lqi65q/good_mcp_design_is_understanding_that_every_tool/) - It's good to think about every response as an opportunity to prompt the model. The model has no memo...

15. [MCP Intent Validation | GenAIScript - Microsoft Open Source](https://microsoft.github.io/genaiscript/blog/mcp-intents/)

16. [[PDF] prompt injection attacks on tool-using llm agents via model context](https://openreview.net/pdf/5b6d6e41167fe1aa8c9744da1e604a594f208f2c.pdf) - Beyond introducing the attack framework, we provide a large-scale empirical evaluation across five M...

17. [[PDF] benchmarking attacks against model context protocol in llm agents](https://openreview.net/pdf/d21258e656430edafce517e6cb61460bfa18b0cf.pdf) - The Model Context Protocol (MCP) standardizes how large language model. (LLM) agents discover, descr...

18. [Using the Model Context Protocol (MCP) for Intent-Based Network ...](https://www.ietf.org/archive/id/draft-zeng-mcp-troubleshooting-00.html) - The Model Context Protocol (MCP) is an open standard that enables Large Language Model (LLM) applica...

19. [Enhancing Model Context Protocol (MCP) with Context-Aware Server Collaboration](https://arxiv.org/abs/2601.11595) - The Model Context Protocol (MCP) (MCP Community, 2025) has emerged as a widely used framework for en...

20. [[PDF] Agent Context Protocols Enhance Collective Inference - OpenReview](https://openreview.net/pdf?id=xKDHZLQJ2O) - Structured communication protocols are critical in enabling collaboration among domain-specific agen...

21. [[PDF] EFFICIENT ON-DEVICE AGENTS VIA ADAPTIVE CONTEXT ...](https://openreview.net/pdf/79d13e5b1f25fe7172b32745ea465ff5abe702e7.pdf) - Our agent matches, or exceeds, the performance of a conventional baseline while dramatically compres...

