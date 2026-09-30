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

import pathlib
import re
import sys

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
ALERTS = ROOT / "observability/alerts.yml"

sys.path.insert(0, str(ROOT / "observability"))
from check_rules import exported_families  # noqa: E402

DERIVED = ("_sum", "_count", "_bucket")

# Recorded series, read from the rules file rather than kept here, so the two
# cannot drift.
RECORDED_PREFIX = "memory:"


def recorded_series() -> set[str]:
    document = yaml.safe_load((ROOT / "observability/recording_rules.yml").read_text())
    return {rule["record"] for group in document["groups"] for rule in group["rules"]}


def check(expr: str, alert: str, known: set[str], failures: list[str]) -> None:
    for metric in re.findall(r"\b(memory_[a-z0-9_]+)", expr):
        base = metric
        for suffix in DERIVED:
            if metric.endswith(suffix) and metric[: -len(suffix)] in known:
                base = metric[: -len(suffix)]
                break
        if base not in known:
            failures.append(f"{alert}: reads `{metric}`, which is not exported")


def main() -> int:
    document = yaml.safe_load(ALERTS.read_text())
    families = exported_families()
    known = families | recorded_series()

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
            # Keyed on a comparison between two bracketed ranges rather than on
            # the presence of `and`: `or vector(0)` is a guard against a series
            # that was never written, not a second window, and flagging those
            # would make the check cry wolf on every well-formed rule.
            ranges = re.findall(r"\[[\dsmhd]+\]", expr)
            if len(ranges) >= 2 and len(set(ranges)) < 2:
                failures.append(
                    f"{name}: a multi-window rule using the same window twice "
                    f"({ranges[0]}); a short second window is what lets it "
                    f"clear when the incident does"
                )

            # Every alert has to be routable.
            if not rule.get("labels", {}).get("severity"):
                failures.append(f"{name}: no `severity` label, so it cannot be routed")

            # And has to say where to look.
            annotations = rule.get("annotations", {})
            if not annotations.get("summary"):
                failures.append(f"{name}: no `summary` to put in a notification")
            if not annotations.get("dashboard"):
                failures.append(
                    f"{name}: no `dashboard` link, so the responder has to go "
                    f"and find the panel themselves"
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
