"""Check the alert rules against the metrics and the SRE method.

`promtool check rules` validates PromQL syntax. It does not validate that an
alert means anything, and the failures that make an alerting setup useless are
mostly semantic:

- A rule that reads a metric the crate does not export evaluates to empty and
  never fires — a safety net that is not there.
- A "multi-window" rule whose two windows are the same query is a single-window
  rule with extra words, and it stays fired long after the incident is over.
- A page on a condition that is a configured setting rather than a fault trains
  people to ignore the channel.
- An alert with no `severity` cannot be routed, so it goes wherever everything
  else goes and loses its urgency on the way.

Run:

    python3 observability/check_alerts.py
"""

import json
import pathlib
import re
import sys

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
ALERTS = ROOT / "observability/alerts.yml"

sys.path.insert(0, str(ROOT / "observability"))
from check_rules import (
    bounded_vocabulary,
    exported_families,
    strip_strings,
    unknown_label_values,
)  # noqa: E402

DERIVED = ("_sum", "_count", "_bucket")

# Recorded series, read from the rules file rather than kept here, so the two
# cannot drift.
RECORDED_PREFIX = "memory:"


def recorded_series() -> set[str]:
    document = yaml.safe_load((ROOT / "observability/recording_rules.yml").read_text())
    return {rule["record"] for group in document["groups"] for rule in group["rules"]}


def _slug(text: str) -> str:
    """A row title as a link fragment, normalised.

    Grafana lowercases the title and turns whitespace into dashes; runs of
    dashes are collapsed here for the same reason the dashboard generator
    collapses them in a uid, so a title containing an em dash compares equal
    whichever side of the collapse a hand-written link landed on. What this
    check is for is a *renamed or deleted* row, and that survives the
    normalisation; the exact dash run a given Grafana version emits is not
    something a rule file can be authoritative about.
    """
    return re.sub(r"-+", "-", re.sub(r"[^a-z0-9]+", "-", text.lower())).strip("-")


def _normalized_anchor(anchor: str) -> str:
    """The anchor as this file compares it.

    The row fragment is dropped (a link may carry `#panel-title`) and both the
    uid and the row are put through [`_slug`], so a link written by hand and one
    derived from the dashboard file land on the same string. A target that is
    not a `/d/uid/row` shape at all is returned as written, which then fails the
    membership test rather than being quietly reinterpreted.
    """
    target = anchor.split("#")[0]
    parts = [part for part in target.split("/") if part]
    if len(parts) == 3 and parts[0] == "d":
        return f"/d/{_slug(parts[1])}/{_slug(parts[2])}"
    return target


def dashboard_anchors() -> set[str]:
    """Every `/d/<uid>/<row>` an alert may point a responder at.

    Derived from the dashboard files rather than kept in a list here — a list
    would go stale the moment a row was renamed, which is exactly the failure
    this catches.
    """
    anchors: set[str] = set()
    for path in sorted((ROOT / "observability/dashboards").glob("*.json")):
        document = json.loads(path.read_text())
        uid = document["uid"]
        for panel in document["panels"]:
            if panel.get("type") != "row":
                continue
            slug = _slug(panel["title"])
            anchors.add(f"/d/{_slug(uid)}/{slug}")
            for child in panel.get("panels", []):
                anchors.add(f"/d/{_slug(uid)}/{slug}#{_slug(child['title'])}")
    return anchors


def check(expr: str, alert: str, known: set[str], failures: list[str]) -> None:
    # Strings first: a label value like `job="memory_mcp"` is a value, not a
    # metric, and scanning it would fail every rule that filters by job.
    for metric in re.findall(r"\b(memory_[a-z0-9_]+)", strip_strings(expr)):
        base = metric
        for suffix in DERIVED:
            if metric.endswith(suffix) and metric[: -len(suffix)] in known:
                base = metric[: -len(suffix)]
                break
        if base not in known:
            failures.append(f"{alert}: reads `{metric}`, which is not exported")

    # A filter on a value the crate cannot emit never matches, so the alert can
    # only ever be silent — the failure nobody notices until it should have
    # paged.
    for value in unknown_label_values(expr, bounded_vocabulary()):
        failures.append(
            f"{alert}: filters on `{value}`, which the crate cannot emit, so "
            f"this rule can never fire"
        )


def main() -> int:
    document = yaml.safe_load(ALERTS.read_text())
    families = exported_families()
    known = families | recorded_series()

    anchors = dashboard_anchors()

    failures: list[str] = []
    alerts = 0

    for group in document["groups"]:
        for rule in group["rules"]:
            alerts += 1
            name = rule["alert"]
            expr = rule["expr"]

            check(expr, name, known, failures)

            # A multi-window rule must actually have two windows. The Workbook's
            # point is that the short one is what makes the alert clear, and a
            # rule that compares one query to itself has no short one.
            #
            # Keyed on a repeated *query*, not on a repeated window string: a
            # rule may legitimately read two different series over the same
            # window — the capture side of `KnowledgeStale` adds MCP calls to
            # watcher revisions at the same `[2h]` — and that is one window
            # applied twice, not a multi-window rule that cannot clear. What
            # the defect looks like is the same selector and range evaluated
            # twice, which is what is flagged here.
            #
            # Also not keyed on the presence of `and`: `or vector(0)` is a
            # guard against a series that was never written, not a second
            # window, and flagging those would make the check cry wolf on every
            # well-formed rule.
            queries = re.findall(r"[A-Za-z_:][A-Za-z0-9_:]*[^\[\]()]*\[[\dsmhd]+\]", strip_strings(expr))
            repeated = sorted({query for query in queries if queries.count(query) > 1})
            if repeated:
                failures.append(
                    f"{name}: a multi-window rule using the same query twice "
                    f"({repeated[0].strip()}); a short second window is what "
                    f"lets it clear when the incident does"
                )

            # Every alert has to be routable.
            if not rule.get("labels", {}).get("severity"):
                failures.append(f"{name}: no `severity` label, so it cannot be routed")

            # And has to say where to look — and the link has to land. A
            # dashboard annotation is the one thing in this file that points
            # outside the repository, so a renamed row or a changed dashboard
            # uid turns it into a 404 that no rule evaluation would ever
            # report, and the responder finds it out at the moment they least
            # want to be looking for it.
            annotations = rule.get("annotations", {})
            if not annotations.get("summary"):
                failures.append(f"{name}: no `summary` to put in a notification")
            target = annotations.get("dashboard")
            if not target:
                failures.append(
                    f"{name}: no `dashboard` link, so the responder has to go "
                    f"and find the panel themselves"
                )
            elif _normalized_anchor(target) not in anchors:
                failures.append(
                    f"{name}: `dashboard: {target}` names no dashboard row that "
                    f"exists; the anchors are derived from the dashboard files, "
                    f"so this is a renamed row or a changed uid"
                )

            # A page on a configuration fact is the failure mode the SRE
            # Workbook warns about in general terms; these two branches are
            # known configuration states rather than faults.
            if rule.get("labels", {}).get("severity") == "page" and re.search(
                r"invite_only|bind_unspecified", expr
            ):
                failures.append(
                    f"{name}: pages on a configured setting; a deployment that is "
                    f"closed to sign-ups would page forever"
                )

            # `or vector(0)` next to a `== 0` test is an always-true rule —
            # unless the metric is guaranteed to exist.
            #
            # The guard exists to make an absent series read as zero, which is
            # right for a *division* and wrong for an *equality against zero*:
            # `… or vector(0) == 0` is `0 == 0`, so the alert fires on every
            # deployment that does not have the series at all. The one real
            # instance of this shipped as a feature-gated rule that fired
            # everywhere the feature was off — the worst shape an alert can
            # have, guaranteed to fire where there is nothing to report.
            #
            # An alert may opt out where its metrics are always present in the
            # scope this file governs. These rules are the HTTP profile's, and
            # the families they read are emitted by that profile's own
            # middleware, so an absent series means "no traffic" rather than
            # "the feature is off" — which is the distinction the guard exists
            # to express. The marker is not a claim that the metric exists in
            # every build.
            if re.search(r"==\s*0", expr) and "or vector(0)" in expr and "unconditional" not in (
                rule.get("labels", {}).get("note", "")
            ):
                failures.append(
                    f"{name}: `or vector(0)` turns an absent series into a "
                    f"literal 0, so testing it `== 0` is true wherever the "
                    f"series does not exist — the alert fires on every "
                    f"deployment without the feature. Drop the guard, or mark "
                    f"the rule `note: unconditional` if the metric is always "
                    f"present wherever this rules file applies"
                )

    print(f"{alerts} alerts across {len(document['groups'])} groups")
    print(f"{len(families)} families, {len(known & set(recorded_series()))} recorded series")
    for failure in failures:
        print(f"  FAIL {failure}")
    if failures:
        return 1
    print("  every alert reads a real metric, is routable, and says where to look")
    return 0


if __name__ == "__main__":
    sys.exit(main())
