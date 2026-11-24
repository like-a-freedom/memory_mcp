# Agent workflow qualification — tool-choice v2

**Decision: NO-GO for this exact local model/runner configuration.**

The fresh v2 registration fixed the schema fingerprint error and used four
new synthetic ordinary families over two preregistered captures. All four
candidate trials in each capture attempted an additional tool call after the
one-call budget. The runner rejected the extra call without executing it and
recorded a budget exit; candidate task success was 0/4, direct reads were
4/4, independent probes were 4/4, and hard violations were 0 in both
captures. `tool_choice: "none"` therefore did not prevent this provider from
returning another tool call.

The [offline qualification](qualification.json) failed the ordinary task and
resource gates. This is a runner/model-control failure, not evidence of a
memory defect or treatment effect. The result motivated the v3 runner change
that removes tool definitions and the associated tool-control fields after
the budget is exhausted.
