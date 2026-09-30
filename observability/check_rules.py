"""Check the recording rules against the metrics this crate actually exports.

`promtool` is not available here — installing it needs a `sudo chown` of the
whole Homebrew prefix, which is not something a test should do — so this
script does what `promtool check rules` would for the failures that matter in
practice: a rule that reads a metric the crate does not export, and a rule
whose `record` name is not the one the dashboards and alerts are written
against.

The metric list is read out of the sources rather than kept here, so it cannot
drift from the code the way a hand-maintained list would.
"""

import pathlib
import re
import sys

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]

# Where a family name can be declared. `include_str!` is how the Rust tests
# find them too, so the two agree by construction.
SOURCES = [
    "crates/memory-mcp/src/shared/observability.rs",
    "crates/memory-mcp/src/observability.rs",
    "crates/memory-mcp/src/knowledge/claims_policy/telemetry.rs",
    "crates/memory-mcp/src/http/registry/provisioning.rs",
    "crates/memory-mcp/src/service/fs_watch/telemetry.rs",
]

# Suffixes the exporter appends to a summary's series. A rule may read any of
# these on a histogram family; they are not separate families.
DERIVED = ("_sum", "_count", "_bucket")


def exported_families() -> set[str]:
    families: set[str] = set()
    for source in SOURCES:
        for line in (ROOT / source).read_text().splitlines():
            code = line.strip()
            if code.startswith("//"):
                continue
            for quoted in re.findall(r'"([^"]*)"', code):
                if (
                    quoted.startswith("memory_")
                    and quoted != "memory_"
                    and not quoted.endswith("_")
                    and all(c.islower() or c.isdigit() or c == "_" for c in quoted)
                ):
                    families.add(quoted)
    return families


def referenced_metrics(expr: str) -> set[str]:
    """Every `memory_*` identifier a PromQL expression reads."""
    return set(re.findall(r"\b(memory_[a-z0-9_]+)", expr))


def main() -> int:
    rules_path = ROOT / "observability/recording_rules.yml"
    document = yaml.safe_load(rules_path.read_text())
    families = exported_families()

    failures: list[str] = []
    recorded: set[str] = set()

    for group in document["groups"]:
        for rule in group["rules"]:
            name = rule["record"]
            recorded.add(name)
            expr = rule["expr"]
            for metric in referenced_metrics(expr):
                base = metric
                for suffix in DERIVED:
                    if metric.endswith(suffix) and metric[: -len(suffix)] in families:
                        base = metric[: -len(suffix)]
                        break
                if base not in families:
                    failures.append(
                        f"{name}: reads `{metric}`, which the crate does not export"
                    )

            # A recording rule that records itself is a cycle.
            if name in referenced_metrics(expr):
                failures.append(f"{name}: reads its own output")

    # A name collision means one rule silently overwrites another.
    if len(recorded) != sum(len(g["rules"]) for g in document["groups"]):
        failures.append("two rules record the same name")

    print(f"{len(families)} families exported, {len(recorded)} rules recorded")
    for failure in failures:
        print(f"  FAIL {failure}")
    if failures:
        return 1
    print("  all rules read metrics this crate exports")
    return 0


if __name__ == "__main__":
    sys.exit(main())
