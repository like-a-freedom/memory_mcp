# Statistics and comparisons

Read when a score will support model choice, a regression/release decision or reliability claims. Match sophistication to the consequence and available data.

## Define the quantity before choosing the test

Specify whether the claim concerns a fixed suite, a sampled task population, a workload stratum or an operational distribution. Hand-curated regression cases support scoped behavioral evidence; their failure fraction is not a deployment incident probability. Separate representative and adversarial distributions.

Record primary metrics, unit, eligible population, baselines, grouping, repetitions, budgets, thresholds/margins, exclusions, uncertainty method and stopping policy before the confirmatory run. Treat post-hoc slices as exploratory; constrain or account for multiplicity when making several confirmatory claims.

Use a pilot or domain variance estimates to plan sample size for a meaningful effect/margin and desired uncertainty. No universal number of tasks or repeats is appropriate. More repeats estimate execution variation, not missing population diversity. Do not tune the final design repeatedly on the same holdout and retain an unseen-test claim.

## Independent unit and aggregation

Repeated calls, turns, paraphrases and shared-source variants are not automatically independent observations. Preserve hierarchical grouping (for example source/root episode → instance → repeat). Choose the independent resampling unit accordingly; simple row bootstrap can understate uncertainty.

Prespecify macro/micro/workload weighting. With multiple job labels, assign one primary family or weights summing to one instead of counting one task several times. Report important slices and base counts so an aggregate cannot hide a material regression. Use deployment weights only when their source supports that distribution.

For predictive ML, use measures appropriate to prevalence, decision costs, probability quality and units. Precision/recall, calibration and utility answer different questions. A synthetic positive fraction does not establish production positive predictive value. For repeated entities or temporal dependence, use a sampling/validation design that respects that dependence.

## Fair comparisons and attribution

Pair baseline and candidate on the same tasks/data/initial states, permissions and environment conditions. Randomize/interleave execution order where drift or provider load matters. Pin versions/settings where possible; unknown versions and provider drift remain limitations. Identical model seeds across providers do not guarantee matched randomness.

Change one factor for attribution, or use a declared factorial design. Comparing different model+harness+interface bundles estimates a bundle difference, not a pure model or MCP effect. Compare common capabilities separately from new coverage; retain unsupported counts. A reference driver checks feasibility, not autonomous-agent usefulness.

Keep resource semantics comparable: total compute/time/context/calls/assistance, relevant concurrency, cold/warm caches and external waiting. Tool-call counts differ with interface granularity; they are not interchangeable units of work. Count discovery, retries, helper agents and human corrections. Report semantic-grader/eval infrastructure cost separately from the evaluated workflow. Use actual token/billing telemetry where available and mark unknown usage; do not convert schema bytes into unmeasured tokens or price.

## Intervals and missingness

Use uncertainty methods that fit the sampling design and outcome: appropriate proportion intervals, paired differences or grouped/cluster resampling. Exact McNemar applies to one prespecified binary outcome per independent matched pair, not every repeated rollout or tool call. A paired bootstrap must retain within-cluster pairs/repeats. Few independent clusters can make a nominal interval unreliable; narrow the conclusion or collect more independent data.

Maintain a scheduled ledger. Inconclusive/invalid/unsupported records cannot disappear from a headline denominator because one condition produced more of them. For outcome-eligible N, let S be established safe successes, F established non-successes and U genuinely unknown outcomes: N=S+F+U. Show S/N and, when useful, identification bounds [S/N,(S+U)/N]. Those bounds are not confidence intervals. Known violations/non-successes do not become unknown due to another grader's error.

Complete-case comparisons may be biased by condition-specific invalids. Resolve missing pairs or perform a predeclared missingness sensitivity analysis before a final noninferiority claim. Report evaluable-only rates as explicitly conditional secondary results. Keep originals when rerunning and apply a prespecified retry policy rather than selecting successful runs.

## Repeated reliability, when relevant

Define success for the intended task: goal fulfillment and appropriate abstention/checkpoint can require separate measures. Repeated evaluation trials start from clean conditions; they do not authorize operational retries.

For n exchangeable observed repeats with s successes and 1≤k≤n:

- pass@k (at least one success): `1 - C(n-s,k)/C(n,k)`.
- pass^k (all k succeed): `C(s,k)/C(n,k)`.
- Let C(a,k)=0 when a<k. Report counts and k; do not publish only the best of several trials.

Under conditionally iid repeats with task-specific success probability p, the corresponding probabilities are 1−(1−p)^k and p^k. Apply taskwise and then aggregate; powers of a pooled mean answer another question. Dependent or adaptive rollouts do not inherit these probability guarantees. pass@k measures coverage with several attempts, not first-attempt reliability or free retry safety.

## Zero observed failures

For zero failures in n independent, representative Bernoulli opportunities, a one-sided 95% exact upper failure-probability bound is `1 - 0.05^(1/n)`. Thus 0/300 gives about 0.994%, not 0%. Other confidence levels or two-sided intervals differ.

This calculation cannot certify deployment risk from correlated, curated or adaptive attack cases. For those suites, say zero found within the stated corpus, threat model and budget. Absence of observed harm is not proof of universal safety.

## Decisions

Choose utility targets, noninferiority margins, risk limits and precision requirements before locked testing. A statistically detectable difference may be practically irrelevant; a useful point estimate may remain too uncertain for the intended decision. Preserve material safety constraints separately from average utility.

If a pilot changes a budget, threshold, oracle or selection rule, version the experimental condition and record comparability. Do not label a newly favorable retrospective threshold as a passed preregistered release gate.
