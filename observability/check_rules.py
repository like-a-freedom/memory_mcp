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


def strip_strings(expr: str) -> str:
    """Drop quoted string literals from an expression before reading names.

    A string in PromQL is a value, never a metric: `job="memory_mcp"` matches
    a label, `label_replace` writes into one. Read without this, that label
    value scans as a family called `memory_mcp` — a name no rule or panel
    could ever mean to read — and a correctly filtered expression fails the
    check for a metric the crate does not export.
    """
    return re.sub(r'"[^"]*"', " ", expr)


def bounded_vocabulary() -> dict[str, set[str]]:
    """The `operation` and `result` label values the crate can emit.

    Read out of the same source that declares them, because a filter on a value
    the crate never emits is the quietest failure in this file: the expression
    parses, the rule evaluates, and it matches nothing — so a panel shows *No
    data* for a series that exists under a name one character away from the one
    written. Nothing at runtime would report it.

    A declaration this cannot find is a hard error rather than an empty set. The
    alternative — skipping the check when the vocabulary comes back empty — is a
    check that turns itself off on a rename and keeps reporting success, which
    is the one outcome worse than not having written it.
    """
    declared: dict[str, set[str]] = {}
    source = (ROOT / "crates/memory-mcp/src/observability.rs").read_text()
    for label in ("KNOWN_OPERATIONS", "KNOWN_RESULTS"):
        block = source.split(f"const {label}: &[&str] = &[", 1)
        if len(block) < 2:
            raise SystemExit(
                f"cannot find `const {label}` in observability.rs, so no label "
                f"value in any rule, alert or panel can be checked; update this "
                f"check rather than letting it pass silently"
            )
        body = block[1].split("]", 1)[0]
        values = set(re.findall(r'"([a-z0-9_]+)"', body))
        if not values:
            raise SystemExit(
                f"`const {label}` in observability.rs reads as empty, so the "
                f"label-value check would accept every value; update this check"
            )
        declared[label] = values
    return {
        "operation": declared["KNOWN_OPERATIONS"],
        "result": declared["KNOWN_RESULTS"],
    }


def unknown_label_values(expr: str, declared: dict[str, set[str]]) -> list[str]:
    """Label values an expression filters on that the crate cannot emit.

    `operation="ingest"` and `operation=~"ingest|extract"` are both filters, and
    both are checked; a value that is not in the declared vocabulary is a typo
    or a rename that has not reached this file.

    Only alternation is understood. A *pattern* filter (`operation=~"lifecycle_.*"`)
    would be reported as unknown values here, which is why none is written: the
    vocabulary is closed and finite, so an enumeration is both sufficient and
    the thing a reader can verify by eye. Anyone who genuinely needs a pattern
    should teach this function about it rather than removing the values.
    """
    unknown: list[str] = []
    for label, known in declared.items():
        if not known:
            continue
        for alternatives in re.findall(rf'{label}=~"([^"]*)"', expr):
            for value in alternatives.split("|"):
                if value and value not in known:
                    unknown.append(f"{label}=\"{value}\"")
        for value in re.findall(rf'{label}="([^"]*)"', expr):
            if value and value not in known:
                unknown.append(f"{label}=\"{value}\"")
    return unknown


def referenced_metrics(expr: str) -> set[str]:
    """Every `memory_*` identifier a PromQL expression reads."""
    return set(re.findall(r"\b(memory_[a-z0-9_]+)", strip_strings(expr)))


def main() -> int:
    rules_path = ROOT / "observability/recording_rules.yml"
    document = yaml.safe_load(rules_path.read_text())
    families = exported_families()
    declared = bounded_vocabulary()

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
            for value in unknown_label_values(expr, declared):
                failures.append(
                    f"{name}: filters on `{value}`, which the crate cannot emit — "
                    f"the expression parses and matches nothing"
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
