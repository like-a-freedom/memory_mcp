"""Generate the two Grafana dashboards.

Both are JSON, and both are generated rather than written by hand. A dashboard
is a few hundred lines of near-identical panel objects; typing them means a typo
in a PromQL selector is invisible in review, and adding a row means copying a
block and hoping every `gridPos` still lines up. Generating them makes the
metric name a parameter, so a rule that changes shows up as a one-line diff.

Run:

    python3 observability/build_dashboards.py

The output is committed, so Grafana imports the same JSON on every machine and
nobody needs this script to read a dashboard. Regenerate after changing a
recorded series name in `recording_rules.yml`.
"""

import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "observability/dashboards"

DATASOURCE = {"type": "prometheus", "uid": "${datasource}"}
REPO = "memory_mcp"
VERSION = 1

# Grafana's 24-column grid. A row is 24 wide and its panels are laid out inside
# it, so the coordinates below are relative to the row.
W_FULL, W_HALF, W_THIRD = 24, 12, 8


def target(expr: str, legend: str, ref_id: str = "A") -> dict:
    """One PromQL query on a panel.

    `legendFormat` is not decoration: with `{{route}}` and `{{outcome}}` in it,
    a panel distinguishes its own lines, and without them a dozen series stack
    into one indistinguishable blob.
    """
    return {
        "datasource": DATASOURCE,
        "editorMode": "code",
        "expr": expr,
        "legendFormat": legend,
        "range": True,
        "refId": ref_id,
    }


def unique_ref_ids(targets: list[dict]) -> list[dict]:
    """One RefId per query on a panel, no matter what `target()` was given.

    Results, query status and per-query transformations are keyed by RefId, so
    two queries called `A` are one query as far as Grafana's machinery is
    concerned — the second one's result is the one the query editor reports on,
    and a query inspector cannot tell them apart. `target()` defaults to `A`,
    so a panel built from three `target(...)` calls shipped three queries named
    `A` until this ran over them.
    """
    used: set[str] = set()
    unique: list[dict] = []
    for index, item in enumerate(targets):
        ref_id = item.get("refId") or ""
        if not ref_id or ref_id in used:
            ref_id = _first_free_ref_id(used, index)
        used.add(ref_id)
        unique.append({**item, "refId": ref_id})
    return unique


def _first_free_ref_id(used: set[str], index: int) -> str:
    for code in range(ord("A"), ord("Z") + 1):
        candidate = chr(code)
        if candidate not in used:
            return candidate
    # Twenty-seven queries on one panel is a bug in the generator, not a
    # layout; keep it unique rather than reusing a RefId the panel already has.
    return f"Q{index}"


def stat(
    title: str,
    expr: str,
    unit: str,
    grid: tuple[int, int, int, int],
    description: str,
    y: int = 0,
    thresholds: list[dict] | None = None,
    decimals: int | None = None,
    links: list[dict] | None = None,
) -> dict:
    """A single number, read at a glance.

    `textMode` and `colorMode` are set explicitly because the defaults are not
    the ones wanted here: a stat that shows a sparkline is a stat nobody can
    read across a row, and one that colours by value paints a healthy service
    green and a broken one the same green.
    """
    panel = {
        "datasource": DATASOURCE,
        "description": description,
        "fieldConfig": {
            "defaults": {
                "unit": unit,
                "color": {"mode": "thresholds"},
                "mappings": [],
            },
            "overrides": [],
        },
        "gridPos": {"h": grid[1], "w": grid[2], "x": grid[0], "y": grid[1] and 0 or 0},
        "id": 0,
        "options": {
            "colorMode": "value",
            "graphMode": "none",
            "justifyMode": "auto",
            "orientation": "auto",
            "reduceOptions": {
                "calcs": ["lastNotNull"],
                "fields": "",
                "values": False,
            },
            "textMode": "auto",
        },
        "pluginVersion": "13.2.4",
        "targets": [target(expr, "")],
        "title": title,
        "type": "stat",
    }
    if thresholds is not None:
        panel["fieldConfig"]["defaults"]["thresholds"] = {
            "mode": "absolute",
            "steps": thresholds,
        }
    else:
        panel["fieldConfig"]["defaults"]["thresholds"] = {
            "mode": "absolute",
            "steps": [{"color": "text", "value": None}],
        }
    if decimals is not None:
        panel["fieldConfig"]["defaults"]["decimals"] = decimals
    if links:
        panel["links"] = links
    return panel


def timeseries(
    title: str,
    targets: list[dict],
    grid: tuple[int, int, int, int],
    description: str,
    y: int = 0,
    unit: str = "short",
    stack: bool = False,
    min_: float | None = 0,
    fill: int = 8,
    legend_calcs: list[str] | None = None,
    links: list[dict] | None = None,
) -> dict:
    """A line or area over time.

    `stack` is off by default and that is deliberate. Grafana's own dashboard
    guide warns that stacking hides data: with a stacked area the middle series
    is not readable, and the top line is a sum that no single query returned. It
    stays available for the two panels where the total genuinely is the point.
    """
    panel = {
        "datasource": DATASOURCE,
        "description": description,
        "fieldConfig": {
            "defaults": {
                "color": {"mode": "palette-classic"},
                "custom": {
                    "axisBorderShow": False,
                    "axisCenteredZero": False,
                    "axisLabel": "",
                    "axisPlacement": "auto",
                    "barAlignment": 0,
                    "drawStyle": "line",
                    "fillOpacity": fill,
                    "gradientMode": "none",
                    "hideFrom": {"legend": False, "tooltip": False, "viz": False},
                    "insertNulls": False,
                    "lineInterpolation": "linear",
                    "lineWidth": 1,
                    "pointSize": 5,
                    "scaleDistribution": {"type": "linear"},
                    "showPoints": "never",
                    "spanNulls": False,
                    "stacking": {
                        "group": "A",
                        "mode": "normal" if stack else "none",
                    },
                    "thresholdsStyle": {"mode": "off"},
                },
                "mappings": [],
                "min": min_,
                "unit": unit,
            },
            "overrides": [],
        },
        "gridPos": {"h": grid[1], "w": grid[2], "x": grid[0], "y": y},
        "id": 0,
        "options": {
            "legend": {
                "calcs": legend_calcs if legend_calcs else ["mean", "max"],
                "displayMode": "table",
                "placement": "right",
                "showLegend": True,
            },
            "tooltip": {"mode": "multi", "sort": "desc"},
        },
        "pluginVersion": "13.2.4",
        "targets": unique_ref_ids(targets),
        "title": title,
        "type": "timeseries",
    }
    if links:
        panel["links"] = links
    return panel


def row(title: str, panels: list[dict]) -> dict:
    """A collapsed row owning the panels beneath it.

    Collapsed, and the panels nested. This is the only arrangement that lays out
    correctly without tracking a running `y` across the whole file:

    - Grafana derives a panel's row from its position on the grid, not from the
      JSON nesting, so an expanded row's panels have to sit *below* it in
      `gridPos` — which means every panel's `y` depends on the height of every
      row before it.
    - Collapsing removes that dependency: the row occupies one grid line and
      its panels are drawn from the row's own `panels` list, wherever they are
      positioned inside it.

    Both files import either way — the JSON is valid either way. The difference
    is that an expanded arrangement with a stale `y` renders as overlapping
    panels, which is exactly what happened before this was collapsed.
    """
    return {
        "collapsed": True,
        "gridPos": {"h": 1, "w": W_FULL, "x": 0, "y": 0},
        "id": 0,
        "panels": panels,
        "title": title,
        "type": "row",
    }


def rows(titles: list[tuple[str, list[dict]]]) -> list[dict]:
    """Stack collapsed rows, each one grid line apart."""
    return [
        {**row(title, panels), "gridPos": {"h": 1, "w": W_FULL, "x": 0, "y": index}}
        for index, (title, panels) in enumerate(titles)
    ]


def text(title: str, body: str, grid: tuple[int, int, int, int], y: int = 0) -> dict:
    return {
        "datasource": DATASOURCE,
        "description": "",
        "gridPos": {"h": grid[1], "w": grid[2], "x": grid[0], "y": y},
        "id": 0,
        "options": {
            "code": {"language": "plaintext", "showLineNumbers": False, "showMiniMap": False},
            "content": body,
            "mode": "markdown",
        },
        "pluginVersion": "13.2.4",
        "title": title,
        "type": "text",
    }


def link(title: str, url_path: str) -> dict:
    """A panel link — to a row on this dashboard, or to the other one."""
    return {"targetBlank": False, "title": title, "url": url_path}


# A link from one dashboard to the other, so neither is a dead end. The
# README claims the pair cross-reference, so it is part of the deliverable
# rather than a nicety.
# (url, title) — in that order, matching Grafana's own field names.
CROSS_LINK = {
    "technical": ("/d/memory_mcp-product", "Product metrics"),
    "product": ("/d/memory_mcp-technical", "Technical metrics"),
}


def technical() -> dict:
    """The on-call dashboard. RED: rate, errors, duration, then saturation.

    The order is the point. A panel that answers "is the service broken" is
    above the fold, and every row below it explains one of those answers. A
    dashboard ordered by subsystem forces the reader to know which subsystem is
    responsible before they know whether anything is.
    """
    sections: list[tuple[str, list[dict]]] = []

    overview = [
        stat(
            "Server errors",
            "sum(memory:http_requests:server_error_rate5m)",
            "reqps",
            (0, 5, 4, 5),
            "Requests per second answered 5xx. The availability signal: a 5xx is "
            "a request the service failed to serve, whatever the client sent. "
            "Read against the traffic panel beside it — a high rate on a quiet "
            "service and a low rate on a busy one are different problems.",
            thresholds=[
                {"color": "green", "value": None},
                {"color": "yellow", "value": 0.01},
                {"color": "red", "value": 0.1},
            ],
            decimals= 3,
            links=[link("Errors over time ↓", "#traffic-and-errors")],
        ),
        stat(
            "p95 latency",
            "memory:http_request_duration:p95_5m_total",
            "s",
            (4, 5, 4, 5),
            "The 95th percentile of request duration across every route, taken "
            "as the worst route rather than an average of percentiles. Averages "
            "of percentiles are not percentiles: one slow endpoint among many "
            "fast ones is exactly the case an average hides.",
            thresholds=[
                {"color": "green", "value": None},
                {"color": "yellow", "value": 0.5},
                {"color": "red", "value": 2},
            ],
            decimals= 3,
            links=[link("Latency by route ↓", "#http-latency")],
        ),
        stat(
            "Traffic",
            "memory:http_requests:rate5m_total",
            "reqps",
            (8, 5, 4, 5),
            "Requests per second served, whole service. Traffic is the "
            "denominator every other rate here is really a fraction of, and a "
            "sudden drop is its own incident: nothing is arriving.",
            thresholds=[
                {"color": "text", "value": None},
            ],
            decimals= 2,
        ),
        stat(
            "Error ratio",
            "memory:http_requests:error_ratio5m",
            "percentunit",
            (12, 5, 4, 5),
            "Server errors over all requests. The figure an SLO is written "
            "against. A 0.1% ratio on a busy service is more failures than a "
            "10% ratio on a quiet one, which is why the rate is beside it "
            "rather than instead of it.",
            thresholds=[
                {"color": "green", "value": None},
                {"color": "yellow", "value": 0.001},
                {"color": "red", "value": 0.01},
            ],
            decimals= 4,
        ),
        text(
            "How to read this row",
            "Four numbers, and they answer one question: **is the service "
            "working?**\n\n"
            "- **Server errors** — requests that failed. If it is red, go to the "
            "*Traffic and errors* row.\n"
            "- **p95 latency** — slow is a failure too, and often the earlier "
            "one. If it is yellow while errors are green, the service is "
            "degrading rather than broken.\n"
            "- **Traffic** — a sudden collapse is its own incident, and no error "
            "panel will show it: a service receiving nothing fails nothing.\n"
            "- **Error ratio** — errors as a fraction of what arrived, which is "
            "what an SLO is stated in.\n\n"
            "Everything below explains one of these four. Read top to bottom "
            "until something is non-zero, then stop — you have found it.",
            (16, 5, 8, 5),
        ),
    ]
    sections.append(("Overview — the four golden signals", overview))

    traffic = [
        timeseries(
            "Requests per second, by route",
            [
                target(
                    "memory:http_requests:rate5m",
                    "{{route}} · {{method}}",
                )
            ],
            (0, 8, 12, 8),
            "Traffic per route, five-minute rate. A route is the router's own "
            "pattern with path parameters collapsed, so this is bounded by what "
            "the server declares — and a path parameter's value never reaches a "
            "label.",
            unit="reqps",
            legend_calcs=["mean", "max", "sum"],
        ),
        timeseries(
            "Responses by status class",
            [
                target(
                    "memory:http_requests:rate5m_errors",
                    "{{outcome}} · {{route}}",
                )
            ],
            (12, 8, 12, 8),
            "Failures per second, split by status class. `4xx` and `5xx` are "
            "kept apart on purpose: a `4xx` is very often the service working "
            "correctly — an unauthenticated request, a bad id — and folding it "
            "into one error number makes a healthy service look broken.",
            unit="reqps",
            legend_calcs=["mean", "max"],
        ),
    ]
    sections.append(("Traffic and errors", traffic))

    latency = [
        timeseries(
            "Latency percentiles over time",
            [
                target("memory:http_request_duration:p50_5m_total", "p50"),
                target("memory:http_request_duration:p95_5m_total", "p95"),
                target("memory:http_request_duration:p99_5m_total", "p99"),
            ],
            (0, 8, 12, 8),
            "p50, p95 and p99 per route, plus the service-wide worst. Read the "
            "gap between p50 and p99: a wide gap means a minority of requests is "
            "very slow, which is a different problem from everything being "
            "uniformly slow.",
            unit="s",
            legend_calcs=["mean", "max", "lastNotNull"],
        ),
        timeseries(
            "p95 by route",
            [target("memory:http_request_duration:p95_5m", "{{route}} · {{outcome}}")],
            (12, 8, 12, 8),
            "The same percentile, one line per route. This is the panel that "
            "answers *which* endpoint — every line here is a router pattern, so "
            "a slow line names the endpoint rather than a class of requests.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
    ]
    sections.append(("HTTP latency", latency))

    saturation = [
        timeseries(
            "Requests in flight",
            [target("memory:http_requests:inflight", "in flight")],
            (0, 7, 12, 7),
            "Requests currently being served. The saturation signal, and an "
            "honest one: the gauge is raised when a request enters a handler and "
            "lowered when it leaves, so a scrape only sees a non-zero value when "
            "it happened to land inside one. **A flat zero is the normal case "
            "and is not evidence of an idle server** — read it as 'no request was "
            "in flight at the instant of the scrape'.",
            unit="short",
            min_=0,
        ),
        text(
            "Why this gauge is mostly zero",
            "It is raised with `+1` on entry to a handler and `-1` on the way "
            "out, with no `await` in between.\n\n"
            "A scrape has to land in that window to see anything. For a service "
            "handling a request per second, most scrapes miss.\n\n"
            "What it *does* tell you, correctly:\n"
            "- A value that never leaves zero across many scrapes, while traffic "
            "is non-zero, means requests are fast enough to miss the window — "
            "not that the server is idle.\n"
            "- A value that *sticks* above zero across consecutive scrapes means "
            "requests are slow enough to be caught, and the number is how many "
            "were in flight at once.\n\n"
            "So: read the trend across scrapes, never a single sample. There is "
            "no process-metric family in this build, so there is no CPU or memory "
            "here to correlate it against — a deliberate gap, not an oversight.",
            (12, 7, 12, 7),
        ),
    ]
    sections.append(("Saturation", saturation))

    jobs = [
        timeseries(
            "Scheduler passes by outcome",
            [target("memory:background_jobs:rate15m", "{{job}} · {{outcome}}")],
            (0, 8, 8, 8),
            "Lease and task scheduler passes, by outcome. `degraded` is its own "
            "value and means the pass itself succeeded while some tenant's step "
            "was refused — neither a failure of the pass nor a healthy one, and "
            "the reason it is not folded into `ok`.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Unhealthy passes",
            [target("memory:background_jobs:unhealthy_rate15m", "{{job}}")],
            (8, 8, 8, 8),
            "Passes that failed or degraded, per scheduler. A steady non-zero "
            "line here is a subsystem that has been failing quietly for a while.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Job duration p95",
            [target("memory:background_job_duration:p95_5m", "{{job}}")],
            (16, 8, 8, 8),
            "How long one pass takes. Timed from when the job started running, "
            "not when it was scheduled, so queue wait is not counted as work.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
    ]
    sections.append(("Background jobs", jobs))

    refusals = [
        timeseries(
            "Refusals by operation",
            [target("memory:runtime_refusals:rate15m", "{{op}}")],
            (0, 8, 12, 8),
            "The HTTP runtime refusing to serve: a quota registry that could not "
            "be reached, a lease that could not be released, a tenant runtime "
            "that would not activate. These answer `5xx` with a generic body, so "
            "a client is told nothing and the log line's request id is the only "
            "way their report reaches an operator. Counted at the one point "
            "every warning passes through, so both logging entry points are "
            "counted exactly once.",
            unit="ops",
            legend_calcs=["mean", "max", "sum"],
        ),
        timeseries(
            "Registry reconciliation",
            [
                target(
                    'sum by (kind) (rate(memory_http_registry_reconciliation_total[15m]))',
                    "{{kind}}",
                )
            ],
            (12, 8, 12, 8),
            "Tenant namespaces reconciliation found wrong: registered but "
            "unbound, or bound to nothing. One increment per affected namespace "
            "per pass, and a pass runs at most once a minute — so a non-zero "
            "rate is a standing inconsistency, not a transient.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
    ]
    sections.append(("Runtime refusals", refusals))

    auth = [
        timeseries(
            "Refusals by branch",
            [target("memory:auth_refusals:rate15m", "{{branch}}")],
            (0, 8, 12, 8),
            "The identity callback declining a sign-in, by reason. Branches are "
            "a fixed set — state mismatch, nonce, id token, missing code, "
            "provider error, and `signup_invite_only`.\n\n"
            "**These are refusals only.** A successful sign-in is not counted, "
            "so this cannot be read as a success rate, and the panel below says "
            "so rather than inviting that reading.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        text(
            "Reading `signup_invite_only`",
            "The sign-up gate answering `403`, and the one branch that is a "
            "*configuration* fact rather than a fault.\n\n"
            "A deployment with `MEMORY_MCP_HTTP_SIGNUP_MODE=invite_only` and no "
            "invitation issued refuses every self-service sign-up — correctly, "
            "and indefinitely. Anyone seeing this branch has usually found the "
            "chicken-and-egg: the administrator door is the way in, and "
            "provisioning the first account through it is a one-time step.\n\n"
            "**A rise here is not an outage.** Every other branch on the left is "
            "worth investigating; this one means either an attack, or a "
            "deployment whose policy closed without anyone noticing.",
            (12, 8, 12, 8),
        ),
    ]
    sections.append(("Authentication", auth))

    stages = [
        timeseries(
            "Stage latency p95",
            [target("memory:pipeline_stage_duration:p95_5m", "{{operation}} · {{stage}}")],
            (0, 8, 12, 8),
            "Where a memory operation spends its time, stage by stage. The five "
            "measured stages are the embedding provider, the vector index, "
            "extraction and the store write.\n\n"
            "An operation's total says *that* something is slow. This says "
            "*which part* — and the candidates need different fixes: a slow "
            "embedding provider is someone else's problem, a slow vector index "
            "is the index, a slow store write is the database.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
        timeseries(
            "Operation latency p95",
            [target("memory:operation_duration:p95_5m", "{{operation}}")],
            (12, 8, 12, 8),
            "The total, per operation, for comparison with the stages beside it. "
            "A stage that accounts for nearly all of the total is the stage to "
            "look at; a set of stages that each account for a little is a "
            "different kind of slow.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
    ]
    sections.append(("Pipeline stages", stages))

    claims = [
        timeseries(
            "Claim pipeline events",
            [target("memory:claim_pipeline:rate15m", "{{stage}} · {{outcome}}")],
            (0, 8, 8, 8),
            "Claim projection and reconciliation, by stage and outcome. "
            "Outcomes worth watching are `supersession` and `contradiction`: a "
            "rise in either means the knowledge graph is finding claims that "
            "disagree, which is the policy working rather than failing.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        stat(
            "Active relations",
            "memory:claim_relations:active",
            "short",
            (8, 8, 4, 8),
            "Claim relations currently stored, across schema and outcome. A "
            "level, read as it stands.\n\n"
            "**This under-reports.** A series only exists once it has been set at "
            "least once, so a schema with no relations of some outcome is absent "
            "from the exposition rather than zero. A sudden drop in the total is "
            "real; a missing contribution is not a measurement.",
            decimals= 0,
        ),
        stat(
            "Mean candidates per slot",
            "memory:claim_candidates:mean15m",
            "short",
            (12, 8, 6, 8),
            "Candidates considered for one claim slot, averaged.\n\n"
            "A **mean, deliberately** — the family is exported as a summary, so a "
            "quantile would read zero for both 'no candidates' and 'a handful', "
            "which is the exact distinction this number exists to show. A rising "
            "mean means reconciliation is scanning more to place each claim.",
            decimals= 2,
        ),
        timeseries(
            "Reconciliation latency p95",
            [
                target(
                    'max(memory_claim_pipeline_duration_seconds{quantile="0.95"})',
                    "p95",
                )
            ],
            (18, 8, 6, 8),
            "How long a claim reconciliation pass takes. Recorded for the "
            "`reconcile` stage; projection is fast enough not to need a panel.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
    ]
    sections.append(("Claims", claims))

    fswatch = [
        stat(
            "Watcher degraded",
            "max(memory_fs_watch_degraded)",
            "short",
            (0, 4, 6, 4),
            "Whether the watcher backend exhausted its retries.\n\n"
            "**A one-way latch.** Once this reads 1 it stays 1 for the process "
            "lifetime — the retry loop has returned and nothing can set it back. "
            "A dashboard that averaged it would report a fraction of a broken "
            "deployment; this reads the value as it stands. A `1` means "
            "filesystem ingestion is off and knowledge from the inbox has "
            "stopped arriving.\n\n"
            "Exported from watcher startup, so a running watcher reads `0` and "
            "reads *No data* only when no watcher ever started.",
            thresholds=[
                {"color": "green", "value": None},
                {"color": "red", "value": 1},
            ],
            decimals=0,
        ),
        timeseries(
            "Revisions by outcome",
            [target("memory:fs_watch_revisions:rate15m", "{{outcome}}")],
            (6, 4, 9, 4),
            "Inbox files processed, by outcome. This is the product's "
            "automatic ingestion path: a rising `failed` count is knowledge that "
            "stopped arriving, and nothing else in this deployment says so.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Retries by stage and reason",
            [target("memory:fs_watch_retries:rate15m", "{{stage}} · {{reason}}")],
            (15, 4, 9, 4),
            "Retries while processing a revision, by the stage that failed and "
            "why. `timeout` is a single revision exceeding its attempt limit; "
            "the failure classes separate a bad file from an unreachable store.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
    ]
    # One band per row. A collapsed row whose children span two bands renders
    # as a diagonal staircase in Grafana once the row sits below the top of the
    # dashboard: the children are laid out in x order, one per line, and the
    # declared second line is lost. So the section's second line of panels
    # becomes a second row instead — checked by check_dashboards.py.
    fswatch_detail = [
        timeseries(
            "Revision latency p95",
            [target("memory:fs_watch_revision_duration:p95_5m", "{{outcome}}")],
            (0, 4, 12, 4),
            "How long one revision takes. A single revision may run to its "
            "attempt timeout, so the upper percentiles can sit well past the "
            "last reported bucket — a high number here is often a timeout, not "
            "a slow success.",
            unit="s",
            legend_calcs=["max", "lastNotNull"],
        ),
        text(
            "Queue depth is not a backlog",
            "`memory_fs_watch_queue_depth` is deliberately absent from this "
            "dashboard.\n\n"
            "It is set **once**, at startup, from a recovery pass, and never "
            "updated again. It is a snapshot of what was queued when the process "
            "started, not the queue as it is now — a backlog that has been "
            "growing for an hour is invisible in it, and a panel showing it "
            "would invite exactly the wrong conclusion.\n\n"
            "There is no live backlog gauge in this build. The honest signal for "
            "a stuck queue is the retry and degraded panels in the row above.\n\n"
            "*These series exist only when the build carries `fs-watch` **and** "
            "the deployment set `MEMORY_INGESTION_INBOX`. Without both, the "
            "panels are empty — which reads as 'off', not 'broken'.*",
            (12, 4, 12, 4),
        ),
    ]
    sections.append(("Filesystem ingestion", fswatch))
    sections.append(("Filesystem ingestion — latency and caveats", fswatch_detail))

    sections.append((
        "Reference",
        [
            text(
                "Metric families on this dashboard",
                "All figures come from recording rules in "
                "`observability/recording_rules.yml`, so a derived number is "
                "computed once per interval rather than once per panel refresh. "
                "Every family is described at its source in "
                "`crates/memory-mcp/src/shared/observability.rs`, and each "
                "description names the trap that family has.\n\n"
                "**The four that will mislead you if read naively:**\n\n"
                "1. `memory_operation_results_total` counts work produced. Its "
                "`rate()` is meaningless; `increase()` over a window is the "
                "question. The product dashboard uses it that way.\n"
                "2. `memory_operation_stock` and `memory_claim_relations_active` "
                "are levels, set rather than accumulated, so they are read as "
                "they stand.\n"
                "3. **Every histogram is exported as a summary, not as "
                "buckets** — there is no `le` label, and latency is read from "
                "the `quantile=\"…\"` lines. Configuring buckets would switch the "
                "exporter process-wide and cost every duration metric its "
                "quantiles.\n"
                "4. `4xx` is not availability. It is very often the service "
                "working correctly.\n\n"
                "**Known gaps, deliberately:**\n\n"
                "- No process metrics (RSS, CPU, uptime) — `metrics-process` is "
                "not a dependency, so there is nothing to correlate a latency "
                "spike against.\n"
                "- No `up` series: Prometheus generates that, not the "
                "application, and this project ships no scrape configuration. "
                "The *Traffic* stat in the overview doubles as a liveness "
                "signal — a zero there with no error means nothing is arriving.\n"
                "- No cross-tenant label. It would be unbounded; the tenant "
                "fingerprint lives in the logs instead.",
                (0, 12, 24, 12),
            )
        ],
    ))

    return dashboard(
        "memory_mcp — technical",
        "RED, from symptom to cause. Starts with the four golden signals and "
        "descends into the subsystem explaining each one. Everything is read "
        "from recording rules, not recomputed per panel. For what the system "
        "holds rather than how it is behaving, see the product dashboard: "
        f"{CROSS_LINK['technical'][0]}.",
        rows(sections),
        tags=["memory_mcp", "technical", "red"],
        cross_link=(CROSS_LINK["technical"][0], CROSS_LINK["technical"][1]),
    )


def product() -> dict:
    """The product owner's dashboard. No percentiles, no RED.

    A product owner asks four questions: is the memory growing, are people
    arriving, what are they doing, and is the knowledge any good. A latency
    percentile answers none of them, so there is none here — a dashboard that
    makes its reader wade through p99 to reach "how much did we learn this week"
    is a dashboard that gets skimmed.
    """
    sections: list[tuple[str, list[dict]]] = []

    stock = [
        stat(
            "Active facts",
            'max(memory_operation_stock{result="active_facts"})',
            "short",
            (0, 5, 4, 5),
            "Facts currently in the knowledge graph, as of the last lifecycle "
            "dashboard read.\n\n"
            "**A level, not a total.** Read the value; a rate here would tell "
            "you how often someone opened a dashboard.",
            decimals= 0,
        ),
        stat(
            "Communities",
            'max(memory_operation_stock{result="communities"})',
            "short",
            (4, 5, 4, 5),
            "Detected communities in the knowledge graph. A rising number with a "
            "flat fact count means the graph is fragmenting — more, smaller "
            "clusters — which is worth knowing before it becomes a search "
            "problem.",
            decimals= 0,
        ),
        stat(
            "Active relations",
            "memory:claim_relations:active",
            "short",
            (8, 5, 4, 5),
            "Claim relations currently stored. The structured layer over the "
            "facts on the left.\n\n"
            "**Under-reports by design**: a series exists only after its first "
            "write, so a schema with no relations of some outcome is absent "
            "rather than zero.",
            decimals=0,
        ),
        stat(
            "Archival candidates",
            'max(memory_operation_stock{result="archival_candidates"})',
            "short",
            (12, 5, 4, 5),
            "Episodes old enough to archive but not yet archived. A standing "
            "non-zero here is a retention policy that is not being run; a "
            "sudden jump is old data arriving all at once.",
            decimals= 0,
        ),
        text(
            "What this row is for",
            "The four numbers a product owner asks about first, all read as "
            "**levels** rather than totals.\n\n"
            "That distinction is the whole reason the stock family exists. These "
            "figures used to be added to a counter, which made the metric the "
            "sum of every inventory ever read: opening the dashboard grew the "
            "number by the size of the store, and its rate reported dashboard "
            "traffic rather than growth in the data.\n\n"
            "A panel reading a *total* here would show knowledge growing every "
            "time someone looked at it.",
            (16, 5, 8, 5),
        ),
    ]
    sections.append(("What exists", stock))

    activity = [
        timeseries(
            "Operations per minute, by operation",
            [target("memory:operation_calls:rate15m", "{{operation}}")],
            (0, 8, 12, 8),
            "Which operations people and agents are actually calling. This is "
            "the adoption signal: a feature nobody calls is a feature nobody "
            "needs, and this is where that shows up before anyone says so.\n\n"
            "Measured over MCP rather than HTTP, so it counts tool calls on the "
            "data plane and not only browser requests.",
            unit="ops",
            legend_calcs=["mean", "max", "sum"],
        ),
        timeseries(
            "Success and failure by operation",
            [
                target(
                    'sum by (operation, outcome) (rate(memory_operation_calls_total[15m]))',
                    "{{operation}} · {{outcome}}",
                )
            ],
            (12, 8, 12, 8),
            "The same operations split by outcome. The default outcome is "
            "`error` and it is set on every early return, so a failure here is "
            "every failure — not only the ones somebody wrote an explicit branch "
            "for.\n\n"
            "A rising `error` line on one operation while the others hold steady "
            "is that operation breaking, not the service.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
    ]
    sections.append(("What people are doing", activity))

    learned = [
        timeseries(
            "Knowledge produced, last 24 hours",
            [target("memory:operation_results:increase1d", "{{result}}")],
            (0, 8, 12, 8),
            "Facts, entities, links and episodes extracted in the last day — the "
            "answer to 'what did this deployment learn'.\n\n"
            "Read as an **increase over a window**, not a rate. A rate of "
            "'facts per second' is a number nobody asks for; 'how much did we "
            "extract today' is the question, and it is the one this panel is "
            "built for.",
            unit="short",
            stack=True,
            legend_calcs=["sum"],
        ),
        timeseries(
            "Knowledge produced, last hour",
            [target("memory:operation_results:increase1h", "{{result}}")],
            (12, 8, 12, 8),
            "The same, over an hour, so a slow day can be told apart from a dead "
            "one. A flat line here with a busy 24-hour panel is the normal shape "
            "of a deployment nobody is using right now.",
            unit="short",
            stack=True,
            legend_calcs=["sum"],
        ),
    ]
    sections.append(("What was learned", learned))

    quality = [
        timeseries(
            "Failure rate by operation",
            [target("memory:operation_calls:error_ratio15m", "{{operation}}")],
            (0, 8, 8, 8),
            "The fraction of calls that failed, per operation. Read against the "
            "traffic panel: a high ratio on a rarely-used operation matters far "
            "less than a low one on the operation everyone depends on.",
            unit="percentunit",
            min_=0,
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Extraction warnings",
            [
                target(
                    'sum(rate(memory_operation_results_total{result="warnings"}[15m]))',
                    "warnings",
                )
            ],
            (8, 8, 8, 8),
            "Warnings raised during extraction, per second. These are the "
            "interesting failures: the operation succeeded, but something about "
            "the input was worth flagging. A flat zero across a busy day is a "
            "sign the warnings are not being raised, not that the input is "
            "clean.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Claim outcomes",
            [
                target(
                    'sum by (outcome) (rate(memory_claim_pipeline_total{stage="reconcile"}[15m]))',
                    "{{outcome}}",
                )
            ],
            (16, 8, 8, 8),
            "What reconciliation found when comparing claims. "
            "`supersession` and `contradiction` are the policy working — two "
            "claims about the same thing that disagree, caught and resolved. A "
            "sudden rise in either is worth a look: it means the knowledge is "
            "becoming less consistent, not more.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
    ]
    sections.append(("Is the knowledge any good", quality))

    access = [
        timeseries(
            "Sign-in refusals by reason",
            [target("memory:auth_refusals:rate15m", "{{branch}}")],
            (0, 8, 12, 8),
            "The identity callback declining a sign-in, by reason.\n\n"
            "**These are refusals only — there is no success counter.** A "
            "completed sign-in is not recorded anywhere in the metrics, so the "
            "share of sign-ins that succeed cannot be computed from this and is "
            "deliberately absent rather than estimated. What is here is the "
            "*reasons people are being turned away*.\n\n"
            "`signup_invite_only` is a policy, not a fault: a deployment closed "
            "to self-service sign-up refuses every one of them, correctly.",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
        timeseries(
            "Filesystem ingestion",
            [target("memory:fs_watch_revisions:rate15m", "{{outcome}}")],
            (12, 8, 12, 8),
            "Episodes arriving automatically from the inbox.\n\n"
            "For a product owner this is the quiet one: when it stops, knowledge "
            "stops accumulating and **nothing in the product says so** — there is "
            "no user-visible error, because no user asked for anything. The "
            "`failed` line growing is knowledge that has stopped arriving.\n\n"
            "*Empty unless the deployment set `MEMORY_INGESTION_INBOX`.*",
            unit="ops",
            legend_calcs=["mean", "max"],
        ),
    ]
    sections.append(("Access and automation", access))

    sections.append((
        "Reference",
        [
            text(
                "How to read this dashboard",
                "**There are no latency percentiles here, on purpose.** This "
                "dashboard answers product questions — is the memory growing, "
                "are people arriving, what are they doing, is the knowledge any "
                "good. A p99 answers none of them, and a dashboard that makes "
                "its reader wade past it to reach the useful number is one that "
                "gets skimmed. Latency lives on the technical dashboard, which "
                "links here.\n\n"
                "**Three figures that mislead if read naively:**\n\n"
                "1. *What exists* reads levels, not totals. These used to be "
                "added to a counter, so the metric was the sum of every "
                "inventory ever read — opening the dashboard grew it by the "
                "size of the store.\n"
                "2. *What was learned* reads an **increase over a window**, not "
                "a rate. The underlying family counts things produced; its rate "
                "is not a quantity anyone asks about.\n"
                "3. *Access and automation* counts **refusals only**. There is "
                "no success counter, so a sign-in success rate is not derivable "
                "from these metrics and is not shown.\n\n"
                "**Known gaps, deliberately:**\n\n"
                "- No per-tenant breakdown. A tenant label would be unbounded in "
                "cardinality; the tenant fingerprint is in the logs instead, "
                "where a high-cardinality field belongs.\n"
                "- No active-user or session metric. Counting them would mean "
                "identifying users, and a metric carrying a user identifier is "
                "a disclosure rather than a measurement.\n"
                "- Filesystem ingestion panels are empty unless the deployment "
                "set `MEMORY_INGESTION_INBOX` — off reads as 'off', not "
                "'broken'.",
                (0, 12, 24, 12),
            )
        ],
    ))

    return dashboard(
        "memory_mcp — product",
        "What the system holds, what people ask it for, and what it learns. No "
        "latency percentiles: those answer an operational question, not a "
        f"product one. For latency, saturation and failures, see the technical "
        f"dashboard: {CROSS_LINK['product'][0]}.",
        rows(sections),
        tags=["memory_mcp", "product"],
        cross_link=(CROSS_LINK["product"][0], CROSS_LINK["product"][1]),
    )


def dashboard(
    title: str,
    description: str,
    panels: list[dict],
    tags: list[str],
    cross_link: tuple[str, str] | None = None,
) -> dict:
    """Wrap panels with the template variables and a text header.

    Grafana's guide is explicit that a dashboard should answer a question, and
    that the answer should be legible without hunting. The header says what the
    dashboard is for; the variable lets one file serve every environment without
    a copy per cluster, which is what keeps the dashboard count at two instead
    of growing with deployments.
    """
    return {
        "annotations": {"list": []},
        "description": description,
        "editable": True,
        "fiscalYearStartMonth": 0,
        "graphTooltip": 1,
        "links": (
            [{"asDropdown": False, "icon": "external link", "includeVars": False,
              "keepTime": True, "tags": [], "targetBlank": False,
              "title": cross_link[1], "type": "link", "url": cross_link[0]}]
            if cross_link
            else []
        ),
        "panels": panels,
        "preload": False,
        "refresh": "1m",
        "schemaVersion": 39,
        "tags": tags,
        "templating": {
            "list": [
                {
                    "current": {},
                    "hide": 0,
                    "includeAll": False,
                    "label": "Data source",
                    "multi": False,
                    "name": "datasource",
                    "options": [],
                    "query": "prometheus",
                    "refresh": 1,
                    "regex": "",
                    "skipUrlSync": False,
                    "type": "datasource",
                }
            ]
        },
        "time": {"from": "now-6h", "to": "now"},
        "timepicker": {},
        "timezone": "browser",
        "title": title,
        # A UID is a URL path segment, so it is restricted to Grafana's
        # allowed characters. Collapsing runs of separators keeps the em dash
        # from leaving a double hyphen behind.
        "uid": re.sub(r"-+", "-", title.lower().replace(" ", "-").replace("—", "")).strip("-"),
        "version": VERSION,
        "weekStart": "",
    }


def count_panels(panels: list[dict]) -> tuple[int, int]:
    """(rows, panels) counting the panels nested inside collapsed rows too."""
    rows = 0
    found = 0
    for panel in panels:
        if panel["type"] == "row":
            rows += 1
            child_rows, child_panels = count_panels(panel.get("panels", []))
            rows += child_rows
            found += child_panels
        else:
            found += 1
    return rows, found


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, built in (("technical.json", technical()), ("product.json", product())):
        path = OUT / name
        path.write_text(json.dumps(built, indent=2, sort_keys=False) + "\n")
        rows, panels = count_panels(built["panels"])
        print(f"{path.relative_to(ROOT)}: {rows} rows, {panels} panels")


if __name__ == "__main__":
    main()
