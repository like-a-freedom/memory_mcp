# Observability

Two Grafana dashboards, forty-two recording rules and eighteen alerts over the
metrics the `streamable-http` profile exports on `/metrics`, plus the scrape
configuration that gets those metrics to a collector in the first place.

Everything here is generated or checked. The dashboards are built by
`build_dashboards.py`, and three checkers stand in for `promtool`, which is
not installed and wants a `sudo chown` of the whole Homebrew prefix to be.
None of them can go stale: each reads the metric list out of the Rust sources
rather than keeping its own copy.

```
observability/
├── recording_rules.yml     42 rules in 11 groups
├── alerts.yml              18 alerts in 7 groups
├── prometheus.yml          scrape + rule_files, for Prometheus
├── vmagent/
│   ├── vmagent.yml         what to scrape, for vmagent
│   └── compose.yml         vmagent + VictoriaMetrics, one command
├── dashboards/
│   ├── technical.json      RED, for whoever is on call
│   └── product.json        for whoever owns the product
├── build_dashboards.py     regenerates the two JSON files
├── check_rules.py          every rule reads a metric and a label value that exist
├── check_alerts.py         every alert is routable and names a real metric
└── check_dashboards.py     every panel reads a series that exists
```

## Getting it running

The application needs no configuration for this to work — `/metrics` is on the
same router as everything else, and the recorder is installed by the
composition root. What ships here is the *other* half: the scrape
configuration, because a metric nothing collects is a metric nothing alerts
on, and the two configurations below are the shapes deployments actually run.

**vmagent + VictoriaMetrics** (the shape a multi-host deployment uses — the
collector scrapes and remote-writes, the store only stores):

```
docker compose -f docker-compose.yml -f observability/vmagent/compose.yml up -d
```

Both files on one compose project share a network, so the target in
`vmagent.yml` — `memory_mcp:8080`, the service name — resolves. Grafana reads
the result as a Prometheus-type data source at `http://localhost:8428`.

To run the collector stack on its own, against a service on the host, the
compose project directory becomes the collector file's own directory and the
config path has to be named relative to it:

```
VMAGENT_CONFIG=./vmagent.yml docker compose -f observability/vmagent/compose.yml up -d
```

Compose resolves a relative bind source against the *project* directory, which
is the directory of the first `-f` file — not the directory of the file the
mount is written in. That is why the mount takes the path as a variable: the
short `- src:dst:ro` form splits on `:` before interpolation runs and compose
then reads the variable as a volume *name* and refuses to start.

**Prometheus alone** (one host, one process):

```
prometheus --config.file=observability/prometheus.yml
```

`rule_files` paths resolve relative to the config file's own directory, so the
two above are named as siblings and Prometheus has to be pointed at this file
rather than at a copy of it elsewhere; its working directory does not matter. The bundled `docker-compose.yml` publishes
`8080`, which is what the scrape target points at; from outside that network it
is `localhost:8080` for a service on the host, or
`host.docker.internal:8080` for a container reaching the host.

Scrape at 15 s in both: the summary window is five minutes, so a shorter
interval buys resolution the window cannot hold, and a longer one misses the
spikes the quantiles are computed over.

Then import the two dashboards (Dashboards → New → Import → upload the JSON).
Alerting is deliberately not configured here — where alerts go is a
deployment's decision (Alertmanager for Prometheus, vmalert for the vmagent
stack), and both consume `alerts.yml` as it stands.

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
| Is anything being collected at all? | technical → *Collection health* |
| What is slow? | technical → *HTTP latency* → *p95 by route* |
| Is it getting slower, or was it always slow? | technical → *HTTP latency* → *Latency percentiles over time* |
| Why is it slow? | technical → *Pipeline stages* |
| Are we saturated? | technical → *Saturation* |
| What is the service refusing, and why? | technical → *Runtime refusals*, *Authentication* |
| Are background jobs keeping up? | technical → *Background jobs* |
| Is knowledge accumulating? | product → *What exists*, *How it is growing*, *What was learned* |
| How old is what it knows? | product → *How fresh is the knowledge* |
| Where is the product dashboard from here? | the dashboard header links to `/d/memory_mcp-product` |
| What are people asking it for? | product → *What people are doing* |
| Is the memory being read, or only written? | product → *Context delivered* |
| Is it learning anything per episode? | product → *What each episode yields* |
| Is what it learned any good? | product → *Is the knowledge any good* |
| Are people arriving, or being turned away? | product → *Access and automation* |

The technical dashboard is ordered RED — rate, errors, duration above the
fold, saturation below — because that is the order an on-call engineer reads
in. Every row below the overview explains one of the four numbers above it.

The product dashboard has **no latency percentiles at all**. Its reader's
questions are whether the memory is growing, how old it is, whether people are
arriving, what they ask it for, and whether the knowledge is any good. A p99
answers none of them, and a dashboard that makes its reader wade past one to
reach "how much did we learn this week" is a dashboard that gets skimmed.

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
| `ScrapeTargetDown` | page | 2m of `up == 0` | — |
| `NoTraffic` | ticket | 15m of zero | — |
| `HighLatency` | page | p95 over 5m above 2s | — |
| `LatencyDegraded` | ticket | p95 over 5m above 1s | — |
| `IngestFailureRate` | page | >10% of ingest failing | — |
| `NoIngestActivity` | ticket | no ingest for 2h | — |
| `KnowledgeStale` | ticket | capture traffic for 2h, nothing landed for 6h | — |
| `WriteOnlyArchive` | ticket | capture for 7d, no recall for 24h | — |
| `FilesystemIngestionStalled` | ticket | nothing processed for 1h | — |
| `WatcherDegraded` | page | the one-way latch | — |
| `BackgroundJobsFailing` | ticket | lease passes unhealthy 15m | — |
| `RuntimeRefusals` | page | >0.1/s for 10m | — |
| `RegistryInconsistent` | ticket | 30m of drift | — |
| `ClaimProjectionFailing` | ticket | 15m of errors | — |
| `SignupsRefused` | ticket | 1h of refusals | — |

Five decisions worth stating, because each could reasonably have gone the
other way:

- **Liveness is two alerts, not one.** `ScrapeTargetDown` reads `up`, which
  the *collector* generates and which therefore exists only where a scrape
  configuration is installed; `NoTraffic` reads the application's own counter,
  which exists everywhere. A dead service reads zero on both, and only `up`
  says which zero it is — while a deployment with no collector at all has no
  `up` series to read, which is why `NoTraffic` stays. Neither replaces the
  other, and the pair is what separates "down" from "idle".
- **`SignupsRefused` is a ticket, not a page.** A deployment closed to
  self-service sign-up refuses every attempt correctly and indefinitely. Paging
  about a configured setting is how a page channel stops being read. It is
  included because it is also what a *user* sees.
- **`WatcherDegraded` reads `max_over_time`, not `avg`.** The gauge is a
  one-way latch, and averaging a step function reports a fraction of a broken
  deployment.
- **Freshness is stamped by the capture path, not derived from a counter.**
  `memory_knowledge_last_write_timestamp_seconds` exists because no derived
  query can answer it: `timestamp()` of a counter that stopped growing reports
  the last *scrape*, and `increase()` reads "nothing new" whether the service
  learned yesterday or has learned nothing for a week. The alternative —
  deriving it in PromQL — was not available, so the stamp is. It is written by
  capture operations only, which is what stops a busy read path from making an
  unwritten store look fresh.
- **`WriteOnlyArchive` uses `unless`, not `== 0` with a guard.** The case that
  matters most is a deployment that has *never* been asked for context, where
  the recall series does not exist at all. `or vector(0)` would turn that
  absence into a zero and then compare it to zero — which fires on every
  deployment where the feature is off, the one shape an alert must never have.

## Nine figures that mislead if read naively

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
`NoTraffic` could never fire. The in-flight gauge is excluded for the same
reason and by the same route check: a scrape is fast enough to be over before
a scrape interval, so a gauge it held at one would be the scrape rate wearing a
load signal's name.

**7. `memory_http_requests_inflight` is mostly zero, correctly.** It is raised
when a request enters a handler and lowered when it leaves, so a scrape only
sees a value when it happened to land inside one — and `/metrics` itself never
raises it, for the reason above. A flat zero means "no request was in flight at
that instant", not "the server is idle". Read the trend across scrapes, never
one sample: a single non-zero is one request caught mid-flight, and a value that
*sticks* across consecutive scrapes is how many were in flight at once.

**8. `memory_knowledge_last_write_timestamp_seconds` is a timestamp.** The age
of the knowledge is `time()` minus it — a difference, in a query or in the rule
that already does it. Read as a duration it is wrong by decades: 1.7 billion
seconds is not "no age", it is a moment in 2026, and a `seconds` unit in a panel
renders it as an elapsed time that looks entirely plausible. It is also absent
until the first completed capture, which means *nothing has ever been learned* —
not zero age.

**9. `memory_auth_signins_total` counts arrivals, not users.** It carries no
label at all, deliberately: an account, subject or tenant label would make it a
disclosure rather than a traffic measure. So a first sign-up and a returning
user are the same number, and active users, activation and retention are not
derivable from it at all. Read it as "someone got in", and read the operation
counters for what they then did.

## Deliberate gaps

Things an operator might look for that are not here, and why.

**No process metrics.** RSS, CPU and uptime are deliberately not exported by
the application. They are host and container figures, not application ones:
`node_exporter` and `cAdvisor` already report them on their own dashboards, and
a second copy inside this exposition would be a number kept in two places to
keep in sync. So a latency spike is correlated against those dashboards rather
than against a panel here, and the *Saturation* row carries what the
application itself can saturate on — in-flight requests — which is the signal
those dashboards do not have.

**No per-tenant label.** A tenant label is unbounded in cardinality — it is one
series per tenant, forever. The tenant fingerprint is in the logs, which is
where a high-cardinality field belongs, and a log line can be read with the
fingerprint in hand.

**No active-user or session metric.** Counting them means identifying users,
and a metric carrying a user identifier is a disclosure rather than a
measurement. `memory_auth_signins_total` is as close as this metrics surface
gets: it counts sign-ins that completed, with no label, which answers "are
people arriving" and nothing about who they are, how often they come back, or
whether their first hour was worth anything.

**`up` is absent without a collector.** Prometheus or vmagent generates it,
never the application, so a deployment that has installed neither of the
scrape configurations above has no `up` series at all: the *Collection health*
row reads *No data*, `ScrapeTargetDown` never evaluates, and `NoTraffic` —
which reads the application's own counter — is the liveness signal that still
works.

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
   and the dashboards are the least of it — the *Collection health* row is the
   same check as a panel, and `ScrapeTargetDown` is it as an alert.

## Verified against a running server

Every claim above was checked against `memory_mcp_http` on an embedded
RocksDB store, serving real requests, rather than reasoned about:

- `route="/health/live"`, `route="/health/ready"`, `route="/mcp"` and
  `route="unmatched"` all appear — the last from a request to a path that does
  not exist, which is the case that has to stay visible.
- **A scrape is not counted.** After a full scrape of the exposition,
  `route="/metrics"` is absent from `memory_http_requests_total`, and
  `memory_http_requests_inflight` reads `0`.
- **Histograms really are summaries**: the duration family carries
  `quantile="0"` … `"1"` lines including the `0.95` the latency panels select,
  plus `_sum` and `_count` — and no `le` label anywhere.
- **`memory_operation_results_total` has no `result="active_facts"`**: the
  lifecycle inventory is a gauge (`memory_operation_stock`), so opening a
  dashboard cannot inflate a counter.
- Every family that appeared carried its `# HELP` line.

The claim families need traffic that exercises claim reconciliation before
they appear, which is why a fresh server's exposition carries the HTTP and
background-job families only.

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

They also check the *label values* an expression filters on, against the
`KNOWN_OPERATIONS` and `KNOWN_RESULTS` vocabulary in `observability.rs`. That
one is quieter than a missing metric: `{operation="extrakt"}` parses, evaluates,
matches nothing, and renders an empty panel that looks exactly like a subsystem
that is off — with no name anywhere that could be wrong.

## Where the metrics are defined

`crates/memory-mcp/src/shared/observability.rs` holds every name, kind, unit and
description. It is the pure kernel — no metrics facade, no exporter — which is
what lets a bounded context say what a measurement means without acquiring
infrastructure, per ADR-0058.

That file is the reference to read when a number on a dashboard is not what you
expected. Every family in its `DESCRIPTIONS` list — twenty-eight of them —
carries a `# HELP` line naming the trap it has, and they render in `/metrics`.
(The checkers count twenty-nine names, because they also pick up a test's own
string; the difference is that string, not a family.)
