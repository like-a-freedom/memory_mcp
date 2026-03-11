# Полная подборка источников по Intent/Skills-Driven MCP Design

## Обзор

Тема проектирования MCP-серверов вокруг пользовательских интентов, а не API-эндпоинтов, оформилась к концу 2025 — началу 2026 года как отдельная дисциплина. Ниже собраны все найденные достоверные и актуальные материалы, разбитые по категориям: от официальных инженерных гайдов Anthropic до enterprise-паттернов, академических публикаций и community-дискуссий.

***

## Официальные гайды Anthropic

### Writing Effective Tools for AI Agents (сентябрь 2025)

Ключевой инженерный пост Anthropic, описывающий полный цикл: прототипирование → eval-driven оптимизация → итерация с агентами. Главные принципы:[^1]

- **Outcomes, not operations** — вместо `list_users` + `list_events` + `create_event` делать `schedule_event`, который сам находит доступность и планирует встречу.[^1]
- **Namespacing** — `asana_projects_search`, `jira_search` вместо generic `search`. Prefix- vs suffix-based namespacing имеет нетривиальный эффект на eval-метрики.[^1]
- **Meaningful context** — возвращать `name`, `image_url`, `file_type` вместо `uuid`, `256px_image_url`, `mime_type`. Замена UUID на human-readable идентификаторы значительно снижает галлюцинации.[^1]
- **Response format enum** — `concise` vs `detailed` по аналогии с GraphQL, позволяет агенту управлять объёмом возвращаемого контекста.[^1]
- **Prompt-engineering descriptions** — описания инструментов — это фактически промпты; даже мелкие изменения в формулировках дают драматические улучшения на eval.[^1]

📎 https://www.anthropic.com/engineering/writing-tools-for-agents

### Code Execution with MCP (ноябрь 2025)

Инженерный пост Anthropic о паттерне «code execution вместо прямых tool calls» для масштабирования. Ключевые идеи:[^2]

- **Progressive disclosure** — инструменты представлены как файлы в файловой системе; агент загружает определения on-demand, а не все 150K токенов сразу.[^2]
- **Context-efficient results** — агент пишет код, который фильтрует 10K строк в execution environment, а модели возвращает 5 отфильтрованных.[^2]
- **Privacy-preserving** — промежуточные данные (PII) остаются в execution environment, не попадая в context window LLM.[^2]
- **Skills как persisted code** — агент сохраняет рабочий код как reusable functions с SKILL.md; это мост между MCP tools и Anthropic Agent Skills.[^2]

📎 https://www.anthropic.com/engineering/code-execution-with-mcp

***

## Практические гайды от индустрии

### Phil Schmid — MCP Best Practices (январь 2026)

Наиболее цитируемый практический гайд. Формулирует 6 правил:[^3]

| # | Правило | Суть |
|---|---------|------|
| 1 | Outcomes, Not Operations | Один инструмент `track_latest_order(email)` вместо трёх |
| 2 | Flatten Arguments | Только примитивы и Literal на верхнем уровне; никаких nested dict |
| 3 | Instructions are Context | Docstrings = промпты; ошибки = guidance |
| 4 | Curate Ruthlessly | 5–15 инструментов на сервер, один сервер — одна задача |
| 5 | Name for Discovery | Паттерн `{service}_{action}_{resource}` |
| 6 | Paginate Large Results | Всегда `has_more`, `next_offset`, `total_count` |

📎 https://www.philschmid.de/mcp-best-practices

### Workato — 9 Patterns for Composable Skills-Based Tool Design (декабрь 2025)

Самый детальный enterprise-кейс с реальными бенчмарками «до/после». На примере hotel management показывается переход от 11 API-обёрток к 2 intent-driven инструментам.[^4]

**Девять паттернов:**

1. **Accept Business Identifiers, Not System IDs** — `guest_email` вместо `contact_id: "003Dn00000QX9fKIAT"`[^4]
2. **Build Idempotency Into Tool Design** — обязательный `idempotency_token` для всех мутирующих операций[^4]
3. **Coordinate State Transitions Atomically** — один `check_in_guest` вместо трёх отдельных обновлений статусов[^4]
4. **Embed Authorization in Tool Design** — `search_cases_on_behalf_of_guest(email)` вместо `search_all_cases`[^4]
5. **Provide Smart Defaults** — агент указывает только переменные параметры[^4]
6. **Document Prerequisites and Error Modes** — предупредить агента о failure modes до вызова[^4]
7. **Support Partial Updates** — «только указанные поля меняются, остальное сохраняется»[^4]
8. **Create Defensive Composition Helpers** — `create_contact_if_not_found(email, ...)` — idempotent prerequisite[^4]
9. **Design for Natural Language Patterns** — имена инструментов совпадают с речью пользователей[^4]

**Бенчмарки (Dewy Resort):**

| Метрика | До | После |
|---------|----|----|
| Response time | 8–12 сек | 2–4 сек |
| Success rate | 73% | 94% |
| Число инструментов | 47 | 12 |
| Tool calls / interaction | 6.2 | 1.8 |

📎 https://www.workato.com/the-connector/designing-skills-for-enterprise-mcp/

### The AI Stack — 5-Step API-to-MCP Conversion (август 2025)

Пошаговый процесс трансформации REST API в MCP tools:[^5]

1. **Consolidate by intent** — `manageUserProfile(action: create|update|delete)` вместо трёх CRUD-эндпоинтов
2. **Agent-guiding responses** — каждый ответ содержит `next_actions` и `suggestion` для агента
3. **Optimize tokens** — batch-операции, `fields` filter, 15–20 инструментов максимум
4. **Permissions and risk bands** — разделение safe/risky, обязательный `confirm_token` для деструктивных операций
5. **Test with MCP Inspector** — мониторинг `tool_selection_frequency`, `error_rate_by_tool`, `avg_response_time`

📎 https://www.theaistack.dev/p/your-api-to-mcp-conversion-is-broken

### Docker — Top 5 MCP Server Best Practices (июль 2025)

Гайд от Docker с перспективы packaging и deployment, но с важным архитектурным тезисом:[^6]

- **Tool Budget** — осознанное управление числом инструментов; «too many tools can discourage adoption»
- **End user = agent** — error messages должны помогать агенту, а не человеку: «API_TOKEN is not valid» вместо «You don't have access»
- **Self-contained tool calls** — каждый вызов создаёт свой connection; latency vs reliability trade-off
- **Document for two audiences** — humans решают какие tools включить; agents используют descriptions для tool selection

📎 https://www.docker.com/blog/mcp-server-best-practices/

***

## Enterprise-архитектура и паттерны

### IBM — Think in Intents, Not APIs (июнь 2025)

Трёхуровневая архитектура MCP-серверов для enterprise:[^7]

| Уровень | Паттерн | Пример |
|---------|---------|--------|
| **Capability-Level** | Goal-Oriented | `diagnoseIncident`, `generateAuditReport` — кросс-продуктовая оркестрация |
| **Product-Level** | System-Oriented | Обёртка стабильных API одного продукта (Instana, Turbonomic) |
| **Component-Level** | Function-Oriented | Отдельные микросервисы, внутреннее использование |

Capability-Level — это «истинный» intent-driven подход: инструменты называются бизнес-глаголами, а не именами API.[^7]

📎 https://gyliu513.github.io/jekyll/update/2025/06/10/mcp-pattern.html

### CodeNinja — Enterprise MCP Deployment Patterns (июнь 2025)

Архитектурный анализ intent-driven систем в enterprise: маппинг intent → policy → execution с учётом compliance и audit requirements.[^8]

📎 https://codeninjaconsulting.com/blog/intent-driven-systems-analysis-model-context-protocol-mcp-enterprise-enviornments

### Agentico — Intent-Based Server for MCP (апрель 2025)

Open-source проект, применяющий концепции Kubernetes (manifests, reconciliation, declarative infrastructure) к управлению MCP-серверами. Определение toolset через `server.yaml`, динамическая загрузка инструментов на основе desired state.[^9]

📎 https://www.reddit.com/r/mcp/comments/1jzf97f/intentbased_server_for_mcp_servers_alpha_release/

***

## Skills vs Tools — архитектурная дифференциация

### Arcade.dev — Skills vs Tools for AI Agents (декабрь 2025)

Глубокий анализ различия между tools (исполняемые функции) и skills (пакетированная экспертиза):[^10]

- **Tools** — «руки» агента: выполняют действия (API, DB, файлы)
- **Skills** — «обучение» агента: контекст, инструкции, доменные знания, поведенческие паттерны
- **Token economics** — один GitHub MCP server = 90+ tools = 50K+ токенов schema; Skills кодируют экспертизу без schema overhead
- **Ключевой тезис** — «от модели всё выглядит одинаково — description + способ invocation; различие важно для архитектуры разработчика, а не для агента»

📎 https://www.arcade.dev/blog/what-are-agent-skills-and-tools/

### Claude Skills vs MCP (декабрь 2025 — февраль 2026)

Skills = filesystem-based modules с YAML frontmatter и Markdown инструкциями; MCP = protocol-based tool interfaces. Оптимальная архитектура — комбинация: MCP серверы добывают данные, Skills интерпретируют и обрабатывают.[^11][^12]

📎 https://dev.to/jimquote/claude-skills-vs-mcp-complete-guide-to-token-efficient-ai-agent-architecture-4mkf
📎 https://intuitionlabs.ai/articles/claude-skills-vs-mcp

***

## Ключевой паттерн: Tool Response as Prompt

### Reddit r/mcp — Every Tool Response is an Opportunity to Prompt the Model (июль 2025)

Пост с 272 upvotes, формулирующий центральную идею: ответы инструментов — это не данные, а промпты для модели:[^13][^14]

- Агент не имеет «памяти» о flow API — каждый ответ должен напоминать следующий шаг
- Успешный ответ: данные + guidance: `"Found 42 active members. Use bulk() to contact them."`
- Ошибки: коррекция вместо стоп-сигнала
- Если tool call работает >90% времени — не нужно перегружать description edge cases; ошибки исправляются через error response

📎 https://www.reddit.com/r/mcp/comments/1lq69b3/good_mcp_design_is_understanding_that_every_tool/

***

## Security и Intent Validation

### Microsoft GenAIScript — MCP Intent Validation (апрель 2025)

Паттерн LLM-as-a-Judge для валидации ответов MCP-инструментов: если tool `weather` возвращает содержимое `package.json` — валидатор блокирует результат до попадания в context.[^15]

📎 https://microsoft.github.io/genaiscript/blog/mcp-intents/

### MCP Security Bench / Prompt Injection Attacks (2025–2026)

Исследования OpenReview показывают, что natural-language metadata инструментов — это дополнительный вектор prompt injection. Атаки типа name-collision, preference manipulation, tool-transfer подчёркивают критичность правильного проектирования descriptions и namespacing.[^16][^17]

📎 https://openreview.net/pdf?id=xKDHZLQJ2O (prompt injection via MCP metadata)
📎 https://openreview.net/pdf?id=91LcHUG0OM (systematic MCP security analysis)

***

## Сетевой домен: Intent-Based Network Management через MCP

### IETF Draft — MCP for Intent-Based Network Troubleshooting (октябрь 2025)

Документ IETF описывает маппинг MCP roles → network management domain:[^18]

- Network devices = MCP servers (expose Resources, Tools, Prompts)
- Network controllers = MCP clients (host LLM в sandbox без прямого L3 доступа к устройствам)
- Intent: «verify reachability between Site-A and Site-B» вместо CLI-команд
- Все взаимодействия через JSON-RPC 2.0, audit trail в syslog

Особенно релевантно для XDR/cybersecurity-контекста: паттерн можно адаптировать для intent-based threat investigation.

📎 https://www.ietf.org/archive/id/draft-zeng-mcp-troubleshooting-00.html

***

## Академические работы и бенчмарки

### Context-Aware MCP с Shared Context Store (январь 2026)

Расширение MCP, где специализированные серверы читают и пишут в общий Shared Context Store, координируясь автономно без повторного prompting. На TravelPlanner benchmark CA-MCP показывает снижение числа LLM-вызовов и частоты failure.[^19]

📎 https://arxiv.org/abs/2601.11595

### Agent Context Protocols (ACPs) — OpenReview

Фреймворк, дополняющий MCP: persistent execution blueprints (DAG с зависимостями sub-tasks) + стандартизированные AGENT_REQUEST/AGENT_RESPONSE schemas + error codes для fault-tolerance. На AssistantBench устанавливает SOTA.[^20]

📎 https://openreview.net/pdf?id=xKDHZLQJ2O

### Efficient On-Device Agents via Adaptive Context (OpenReview)

Оптимизация для edge-deployment: минималистичная сериализация tool schemas + just-in-time schema loading (полные определения только при выборе инструмента) → 6x сокращение context с 3B SLM.[^21]

📎 https://openreview.net/pdf/79d13e5b1f25fe7172b32745ea465ff5abe702e7.pdf

***

## Сводная таблица источников

| Источник | Автор / Организация | Дата | Фокус | Достоверность |
|----------|-------------------|------|-------|---------------|
| Writing Effective Tools for AI Agents | Anthropic Engineering | Сен 2025 | Eval-driven tool design | ★★★★★ |
| Code Execution with MCP | Anthropic Engineering | Ноя 2025 | Progressive disclosure, Skills | ★★★★★ |
| MCP Best Practices | Phil Schmid | Янв 2026 | 6 практических правил | ★★★★☆ |
| 9 Patterns for Composable MCP | Workato (Zaynel Taheri) | Дек 2025 | Enterprise skills-based design | ★★★★☆ |
| Think in Intents, Not APIs | IBM (Guangya Liu, Ruchi Mahindru) | Июн 2025 | Трёхуровневая архитектура | ★★★★☆ |
| Skills vs Tools for AI Agents | Arcade.dev | Дек 2025 | Architectural differentiation | ★★★★☆ |
| Docker MCP Best Practices | Docker | Июл 2025 | Packaging + tool budget | ★★★★☆ |
| 5-Step API-to-MCP Conversion | The AI Stack | Авг 2025 | Пошаговый conversion guide | ★★★☆☆ |
| Tool Response as Prompt | Reddit r/mcp | Июл 2025 | Community pattern | ★★★☆☆ |
| IETF MCP Network Troubleshooting | IETF Draft | Окт 2025 | Network/security domain | ★★★★☆ |
| MCP Intent Validation | Microsoft GenAIScript | Апр 2025 | Security pattern | ★★★☆☆ |
| Context-Aware MCP (CA-MCP) | arxiv | Янв 2026 | Shared Context Store | ★★★☆☆ |
| Agentico Intent-Based Server | La Rebelion Labs | Апр 2025 | K8s-like MCP orchestration | ★★☆☆☆ |

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

12. [Claude Skills vs. MCP: A Technical Comparison for AI Workflowsintuitionlabs.ai › articles › claude-skills-vs-mcp](https://intuitionlabs.ai/articles/claude-skills-vs-mcp) - Learn the key differences between Anthropic's Claude Skills and the Model Context Protocol (MCP). Th...

13. [Good MCP design is understanding that every tool response is an opportunity to prompt the model](https://www.reddit.com/r/mcp/comments/1lq69b3/good_mcp_design_is_understanding_that_every_tool/) - Good MCP design is understanding that every tool response is an opportunity to prompt the model

14. [Good MCP design is understanding that every tool response is an ...](https://www.reddit.com/r/LLMDevs/comments/1lqi65q/good_mcp_design_is_understanding_that_every_tool/) - It's good to think about every response as an opportunity to prompt the model. The model has no memo...

15. [MCP Intent Validation | GenAIScript - Microsoft Open Source](https://microsoft.github.io/genaiscript/blog/mcp-intents/)

16. [[PDF] prompt injection attacks on tool-using llm agents via model context](https://openreview.net/pdf/5b6d6e41167fe1aa8c9744da1e604a594f208f2c.pdf) - Beyond introducing the attack framework, we provide a large-scale empirical evaluation across five M...

17. [[PDF] benchmarking attacks against model context protocol in llm agents](https://openreview.net/pdf/d21258e656430edafce517e6cb61460bfa18b0cf.pdf) - The Model Context Protocol (MCP) standardizes how large language model. (LLM) agents discover, descr...

18. [Using the Model Context Protocol (MCP) for Intent-Based Network ...](https://www.ietf.org/archive/id/draft-zeng-mcp-troubleshooting-00.html) - The Model Context Protocol (MCP) is an open standard that enables Large Language Model (LLM) applica...

19. [Enhancing Model Context Protocol (MCP) with Context-Aware Server Collaboration](https://arxiv.org/abs/2601.11595) - The Model Context Protocol (MCP) (MCP Community, 2025) has emerged as a widely used framework for en...

20. [[PDF] Agent Context Protocols Enhance Collective Inference - OpenReview](https://openreview.net/pdf?id=xKDHZLQJ2O) - Structured communication protocols are critical in enabling collaboration among domain-specific agen...

21. [[PDF] EFFICIENT ON-DEVICE AGENTS VIA ADAPTIVE CONTEXT ...](https://openreview.net/pdf/79d13e5b1f25fe7172b32745ea465ff5abe702e7.pdf) - Our agent matches, or exceeds, the performance of a conventional baseline while dramatically compres...

