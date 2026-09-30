# Observability

Two Grafana dashboards, thirty recording rules and fifteen alerts over the
metrics the `streamable-http` profile exports on `/metrics`.

Everything here is generated or checked. The dashboards are built by
`build_dashboards.py`, and three checkers stand in for `promtool`, which is
not installed and wants a `sudo chown` of the whole Homebrew prefix to be.
None of them can go stale: each reads the metric list out of the Rust sources
rather than keeping its own copy.

```
observability/
├── recording_rules.yml     30 rules in 9 groups
├── alerts.yml              15 alerts in 6 groups
├── dashboards/
│   ├── technical.json      RED, for whoever is on call
│   └── product.json        for whoever owns the product
├── build_dashboards.py     regenerates the two JSON files
├── check_rules.py          every rule reads a metric the crate exports
├── check_alerts.py         every alert is routable and names a real metric
└── check_dashboards.py     every panel reads a series that exists
```

## Getting it running

The application needs no configuration for this to work — `/metrics` is on the
same router as everything else, and the recorder is installed by the
composition root. What this directory does not ship is the scrape
configuration, because scrape targets are a property of a deployment and
guessing one here would be wrong more often than right.

A minimal `prometheus.yml` for the bundled `docker-compose.yml`, with
Prometheus running on the host:

```yaml
scrape_configs:
  - job_name: memory_mcp
    static_configs:
      - targets: ["localhost:8080"]
```

`docker-compose.yml` publishes `8080`, so the exposition is already reachable
from the host. If Prometheus itself runs in a container, use
`host.docker.internal:8080` on macOS and Windows, or the compose service name
`memory_mcp:8080` when both are on the same network.

Scrape at 15 s: the summary window is five minutes, so a shorter interval buys
resolution the window cannot hold, and a longer one misses the spikes the
quantiles are computed over.

Then import the two dashboards (Dashboards → New → Import → upload the JSON),
and add `observability/recording_rules.yml` and `observability/alerts.yml` to
`rule_files`.

Regenerating the dashboards after changing a rule name:

```
python3 observability/build_dashboards.py
python3 observability/check_rules.py
python3 observability/check_dashboards.py
python3 observability/check_alerts.py
```

## Which dashboard answers which question

| Question | Panel |
|---|---|
| Is the service working right now? | technical → *Overview — the four golden signals* |
| What is slow? | technical → *HTTP latency* → *p95 by route* |
| Is it getting slower, or was it always slow? | technical → *HTTP latency* → *Latency percentiles over time* |
| Why is it slow? | technical → *Pipeline stages* |
| Are we saturated? | technical → *Saturation* |
| What is the service refusing, and why? | technical → *Runtime refusals*, *Authentication* |
| Are background jobs keeping up? | technical → *Background jobs* |
| Is knowledge accumulating? | product → *What exists*, *What was learned* |
| Where is the product dashboard from here? | the dashboard header links to `/d/memory_mcp-product` |
| What are people asking it for? | product → *What people are doing* |
| Is what it learned any good? | product → *Is the knowledge any good* |
| Why can nobody sign in? | product → *Access and automation* |

The technical dashboard is ordered RED — rate, errors, duration above the
fold, saturation below — because that is the order an on-call engineer reads
in. Every row below the overview explains one of the four numbers above it.

The product dashboard has **no latency percentiles at all**. Its reader's
questions are whether the memory is growing, whether people are arriving, what
they are doing and whether the knowledge is any good. A p99 answers none of
them, and a dashboard that makes its reader wade past one to reach "how much
did we learn this week" is a dashboard that gets skimmed.

## The SLO

**99.9% availability over 30 days**, stated here because the burn-rate alerts
below are meaningless without it: a burn rate is a fraction of an error budget,
and the budget comes from the SLO. Changing this number means changing the
thresholds in `alerts.yml` to match — they are written as `burn × 0.001`, where
`0.001` is the one-in-a-thousand budget.

Whether 99.9% is the right number is a product decision, not an engineering
one, and it is recorded here so the argument has somewhere to happen.

## The alerts, and why they are shaped this way

They use the multiwindow multi-burn-rate method from the SRE Workbook's
*Alerting on SLOs* chapter, which is the one that chapter settles on.

The obvious alternative does not work. Alerting when the recent error rate
exceeds the SLO fires **up to 144 times a day** at 99.9%, almost all of it
noise: 0.1% errors sustained for ten minutes consumes 0.000023% of a
thirty-day budget — about one forty-thousandth of what there is to spend. An on-call engineer who learns that page is noise stops reading pages,
including the one that mattered.

So each SLO alert pairs a long window, which decides whether to fire, with a
short one, which decides whether to stay fired. The short window is what stops
an alert outliving its incident by the length of the long window.

| Alert | Severity | Windows | Budget |
|---|---|---|---|
| `HighErrorRate` | page | 1h and 5m at 14.4× | 2% |
| `SustainedErrorRate` | page | 6h and 30m at 6× | 5% |
| `ErrorBudgetBurn` | ticket | 3d and 6h at 1× | 10% |
| `NoTraffic` | ticket | 15m of zero | — |
| `HighLatency` | page | p95 over 5m above 2s | — |
| `LatencyDegraded` | ticket | p95 over 5m above 1s | — |
| `IngestFailureRate` | page | >10% of ingest failing | — |
| `NoIngestActivity` | ticket | no ingest for 2h | — |
| `FilesystemIngestionStalled` | ticket | nothing processed for 1h | — |
| `WatcherDegraded` | page | the one-way latch | — |
| `BackgroundJobsFailing` | ticket | lease passes unhealthy 15m | — |
| `RuntimeRefusals` | page | >0.1/s for 10m | — |
| `RegistryInconsistent` | ticket | 30m of drift | — |
| `ClaimReconciliationFailing` | ticket | 15m of errors | — |
| `SignupsRefused` | ticket | 1h of refusals | — |

Three decisions worth stating, because each could reasonably have gone the
other way:

- **No `BackendDown`.** There is no `up` series here: Prometheus generates
  that, not the application, and this project ships no scrape configuration.
  `NoTraffic` is the substitute — no requests *and* no errors means either idle
  or unreachable, and both are worth knowing about.
- **`SignupsRefused` is a ticket, not a page.** A deployment closed to
  self-service sign-up refuses every attempt correctly and indefinitely. Paging
  about a configured setting is how a page channel stops being read. It is
  included because it is also what a *user* sees.
- **`WatcherDegraded` reads `max_over_time`, not `avg`.** The gauge is a
  one-way latch, and averaging a step function reports a fraction of a broken
  deployment.

## Six figures that mislead if read naively

Each of these is a place where the obvious query returns a number that is
plausible and wrong. Every one is also stated in the metric's own `# HELP`
line, which is where someone reading `/metrics` will meet it first.

**1. Every histogram is a summary, not buckets.** The Prometheus exporter
switches to `_bucket` exposition *process-wide* when buckets are configured,
which would cost every duration metric its quantile series — so buckets are
not configured. A scrape therefore carries `quantile="0.95"` lines, a `_sum`, and
a `_count`. There is no `le` label, and a panel or rule filtering on one
returns nothing at all.

The summary window is **five minutes**, configured explicitly. The exporter
defaults to three buckets of twenty seconds — about a minute — which is short
enough that a percentile decays to nothing within a minute of the last
request. A sustained regression would read as a series of spikes, and a long
one cannot be reconstructed at all because the observations behind it are gone.
Every latency rule is named `_5m` because that is the window it actually has.

**2. `memory_operation_results_total` counts work produced, not a level.**
`rate()` on it is meaningless — "facts per second" is a number nobody asks for.
The product dashboard reads `increase()` over a window, which answers "how much
did we learn today".

**3. `memory_operation_stock` and `memory_claim_relations_active` are levels.**
They are set rather than accumulated, so they are read as they stand. The
lifecycle dashboard's inventory used to go into a counter, which made the
metric the sum of every inventory ever read: opening the dashboard added the
size of the store, and its rate reported dashboard traffic rather than growth in
the data.

**4. `memory_claim_candidates_considered` must be read as a mean.** It is a
count, not a duration, so a quantile is useless: everything below the first
reported quantile collapses to zero, which makes "no candidates" and "a
handful" the same number. The mean is `_sum / _count`.

Its name deliberately does **not** end in `_count`. The exporter appends
`_count` to a summary's count line only when the name does not already end in
it, so a family named `…_candidate_count` keeps its count on the *bare* name —
the name its seven quantile lines carry — and a selector for the bare name
sums all eight series. That made the mean 0.14 where the answer was 4.0. A
test in the crate pins the name.

**5. `4xx` is not availability.** It is very often the service working
correctly: an unauthenticated request, a client sending a bad id. Folding it
into one "error" number makes a healthy service look broken, which is why the
traffic panel splits the two and the error ratio counts `5xx` alone.

**6. A scrape is not traffic.** `/metrics` is on the same router and the access
log wraps the whole router, so a scrape arrives there like any other request. It
is logged and *not counted*: at a 15-second interval that is four requests a
minute, forever, in the counter every traffic figure and error ratio is
computed from — which would mean an idle deployment never reads zero and
`NoTraffic` could never fire, and the in-flight gauge would be pinned at or
above one whenever a scrape is in flight.

**7. `memory_http_requests_inflight` is mostly zero, correctly.** It is raised
when a request enters a handler and lowered when it leaves, with no `await` in
between, so a scrape only sees a value when it happened to land inside one. A
flat zero means "no request was in flight at that instant", not "the server is
idle". Read the trend across scrapes, never one sample.

## Deliberate gaps

Things an operator might look for that are not here, and why.

**No process metrics.** RSS, CPU and uptime come from the `metrics-process`
crate, which is not a dependency. So there is nothing to correlate a latency
spike against, and the *Saturation* row is the only saturation signal there is.

**No per-tenant label.** A tenant label is unbounded in cardinality — it is one
series per tenant, forever. The tenant fingerprint is in the logs, which is
where a high-cardinality field belongs, and a log line can be read with the
fingerprint in hand.

**No active-user or session metric.** Counting them means identifying users,
and a metric carrying a user identifier is a disclosure rather than a
measurement.

**No `up` series.** See *No `BackendDown`* above.

**Filesystem ingestion panels are empty unless the feature is on.** The
`memory_fs_watch_*` families exist only when the build carries `fs-watch` **and**
the deployment set `MEMORY_INGESTION_INBOX`. Without both, those panels show
nothing — which reads as "off", not "broken", and the dashboards say so on the
panels themselves.

**`memory_fs_watch_queue_depth` is deliberately not plotted.** It is set once,
at startup, from a recovery pass, and never updated again. It is a snapshot of
what was queued when the process started, not the queue as it is now, and a
backlog that has been growing for an hour is invisible in it. A panel showing
it would invite exactly the wrong conclusion. There is no live backlog gauge in
this build; the honest signal for a stuck queue is the retry and degraded
panels.

**`memory_claim_relations_active` under-reports.** A series appears only after
its first write, so a schema with no relations of some outcome is absent from
the exposition rather than zero. A sudden drop in the total is real; a missing
contribution is not a measurement.

## When a panel is empty

In order of likelihood:

1. **The rule has not been evaluated yet.** Recording rules run on their group's
   interval, 30 s. A freshly started Prometheus shows gaps until each rule has
   fired once.
2. **The feature is off.** Filesystem ingestion needs `fs-watch` and
   `MEMORY_INGESTION_INBOX`; the HTTP families need the `streamable-http`
   profile. An absent series means the feature is off, not that it is idle.
3. **The series has never been written.** Prometheus series are created lazily
   per label set, so `outcome="5xx"` does not exist on a service that has never
   returned a 5xx. The rules that care use `or vector(0)`, so a recorded rule
   shows a flat zero rather than a gap — but a *raw* query will show nothing.
4. **The scrape is not reaching the service.** `curl http://<host>:8080/metrics`
   should return an exposition. If it does not, nothing downstream of it works
   and the dashboards are the least of it.

## Checking the configuration

```
python3 observability/check_rules.py       # rules read real metrics
python3 observability/check_dashboards.py  # panels read real series
python3 observability/check_alerts.py      # alerts are routable and real
```

Each exits non-zero and names the offending line. They are not a substitute for
`promtool check rules` — that also validates PromQL syntax, which these do not —
but they catch the failures that matter more in practice: a rule or panel
naming a series that does not exist renders empty, and an empty panel is
indistinguishable from a subsystem that is switched off.

## Where the metrics are defined

`crates/memory-mcp/src/shared/observability.rs` holds every name, kind, unit and
description. It is the pure kernel — no metrics facade, no exporter — which is
what lets a bounded context say what a measurement means without acquiring
infrastructure, per ADR-0058.

That file is the reference to read when a number on a dashboard is not what you
expected. Each of the twenty-five families carries a `# HELP` line naming the
trap it has, and they render in `/metrics`.
