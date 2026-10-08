"""Check the dashboards against the recording rules.

A panel that names a recorded series which does not exist renders empty, and an
empty panel is indistinguishable from a subsystem that is off: the reader sees
"no failures" when the truth is "this was never measured". Nothing in Grafana
reports the difference, and a dashboard is exactly where that mistake survives
longest — it is a file nobody re-reads.

So the check is mechanical: every `memory:*` identifier a panel reads must be
something `recording_rules.yml` records, or a raw `memory_*` metric the crate
exports. Anything else is a typo, a rule that was renamed, or a panel left
behind after its rule was deleted.

Run:

    python3 observability/check_dashboards.py
"""

import json
import pathlib
import re
import sys
from dataclasses import dataclass

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
RULES = ROOT / "observability/recording_rules.yml"
DASHBOARDS = ROOT / "observability/dashboards"
EXTERNAL_METRICS = ROOT / "observability/external_metrics.yml"


@dataclass(frozen=True)
class MetricSpec:
    exporter: str
    required_matchers: dict[str, str]
    allowed_matchers: dict[str, frozenset[str]]


@dataclass(frozen=True)
class ExternalMetricContract:
    metrics: dict[str, MetricSpec]


EXTERNAL_METRIC_NAME = re.compile(
    r"\b(?:container_memory_[a-zA-Z0-9_]+|node_memory_[a-zA-Z0-9_]+)"
)
EXTERNAL_MATCHER = re.compile(
    r'\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*(=|!=|=~|!~)\s*("(?:[^"\\\\]|\\\\.)*")'
)
EXTERNAL_NAME_SELECTOR = re.compile(
    r'__name__\s*(=|!=|=~|!~)\s*("(?:[^"\\\\]|\\\\.)*")'
)
ZERO_VECTOR_FALLBACK = re.compile(r"\bor\s+vector\s*\(\s*0(?:\.0+)?\s*\)", re.IGNORECASE)
LEGEND_LABEL = re.compile(r"\s*(?:\$labels\.)?([a-zA-Z_][a-zA-Z0-9_]*)\s*")
IDENTIFYING_EXTERNAL_LABELS = frozenset(
    {
        "cluster",
        "container",
        "container_id",
        "container_name",
        "host",
        "hostname",
        "id",
        "instance",
        "machine",
        "machine_id",
        "name",
        "namespace",
        "node",
        "node_name",
        "pod",
        "pod_name",
        "tenant",
        "tenant_id",
    }
)


def load_external_metric_contract(
    path: pathlib.Path = EXTERNAL_METRICS,
) -> ExternalMetricContract | None:
    if not path.is_file():
        return None

    document = yaml.safe_load(path.read_text())
    if (
        not isinstance(document, dict)
        or set(document) != {"metrics"}
        or not isinstance(document["metrics"], list)
        or not document["metrics"]
    ):
        raise ValueError("external metric contract has an invalid schema")

    metrics: dict[str, MetricSpec] = {}
    for entry in document["metrics"]:
        if not isinstance(entry, dict) or set(entry) != {
            "name",
            "exporter",
            "required_matchers",
            "allowed_matchers",
        }:
            raise ValueError("external metric contract has an invalid schema")

        name = entry["name"]
        exporter = entry["exporter"]
        required_matchers = entry["required_matchers"]
        allowed_matchers = entry["allowed_matchers"]
        expected_exporter = None
        if isinstance(name, str):
            if name.startswith("container_memory_"):
                expected_exporter = "cadvisor"
            elif name.startswith("node_memory_"):
                expected_exporter = "node_exporter"
        if (
            not isinstance(name, str)
            or EXTERNAL_METRIC_NAME.fullmatch(name) is None
            or expected_exporter is None
            or exporter != expected_exporter
            or name in metrics
            or not isinstance(required_matchers, dict)
            or not required_matchers
            or not isinstance(allowed_matchers, dict)
        ):
            raise ValueError("external metric contract has an invalid schema")

        normalized_allowed: dict[str, frozenset[str]] = {}
        for label, values in allowed_matchers.items():
            if (
                not isinstance(label, str)
                or re.fullmatch(r"[a-zA-Z_][a-zA-Z0-9_]*", label) is None
                or label in IDENTIFYING_EXTERNAL_LABELS
                or not isinstance(values, (list, set))
                or not values
                or any(not isinstance(value, str) or not value for value in values)
            ):
                raise ValueError("external metric contract has an invalid selector")
            normalized_allowed[label] = frozenset(values)

        if any(
            not isinstance(label, str)
            or not isinstance(value, str)
            or label not in normalized_allowed
            or value not in normalized_allowed[label]
            for label, value in required_matchers.items()
        ):
            raise ValueError("external metric contract has an invalid selector")

        metrics[name] = MetricSpec(
            exporter=exporter,
            required_matchers=required_matchers,
            allowed_matchers=normalized_allowed,
        )

    return ExternalMetricContract(metrics=metrics)



sys.path.insert(0, str(ROOT / "observability"))
from check_rules import (
    bounded_vocabulary,
    exported_families,
    strip_strings,
    unknown_label_values,
)  # noqa: E402


def recorded_series() -> set[str]:
    document = yaml.safe_load(RULES.read_text())
    return {rule["record"] for group in document["groups"] for rule in group["rules"]}


def panel_expressions(panel: dict) -> list[str]:
    """Every PromQL expression on a panel, including inside collapsed rows."""
    if isinstance(panel.get("panels"), list):
        return [
            expr
            for child in panel["panels"]
            for expr in panel_expressions(child)
        ]
    return [target.get("expr", "") for target in panel.get("targets", [])]


def panel_titles(panel: dict) -> list[tuple[str, str]]:
    """(title, expr) for this panel and any nested ones, for a failure message."""
    if isinstance(panel.get("panels"), list):
        return [pair for child in panel["panels"] for pair in panel_titles(child)]
    return [
        (panel.get("title", "<untitled>"), target.get("expr", ""))
        for target in panel.get("targets", [])
    ]


def _external_selector(
    expr: str, start: int
) -> tuple[dict[str, tuple[str, str]] | None, str | None]:
    selector = re.match(r"\s*\{([^{}]*)\}", expr[start:])
    if selector is None:
        return None, "external metric requires a literal selector"

    body = selector.group(1)
    matchers: dict[str, tuple[str, str]] = {}
    cursor = 0
    while cursor < len(body):
        matcher = EXTERNAL_MATCHER.match(body, cursor)
        if matcher is None:
            return None, "external metric selector has unsupported syntax"
        label, operator, quoted_value = matcher.groups()
        try:
            value = json.loads(quoted_value)
        except json.JSONDecodeError:
            return None, "external metric selector has an invalid string value"
        if label in matchers:
            return None, f"external metric selector repeats matcher `{label}`"
        matchers[label] = (operator, value)
        cursor = matcher.end()
        if cursor < len(body):
            if body[cursor] != ",":
                return None, "external metric selector requires comma-separated matchers"
            cursor += 1
            if cursor == len(body):
                return None, "external metric selector has unsupported syntax"
    return matchers, None


def validate_external_expression(
    expr: str, contract: ExternalMetricContract | None
) -> list[str]:
    errors = []
    for name_match in EXTERNAL_NAME_SELECTOR.finditer(expr):
        operator, quoted_name = name_match.groups()
        try:
            selected_name = json.loads(quoted_name)
        except json.JSONDecodeError:
            continue
        if (
            operator != "="
            or "container_memory_" in selected_name
            or "node_memory_" in selected_name
        ):
            errors.append(
                "external metric family selector via `__name__` is unsupported"
            )

    scanned = strip_strings(expr)
    has_zero_fallback = ZERO_VECTOR_FALLBACK.search(scanned) is not None
    for match in EXTERNAL_METRIC_NAME.finditer(scanned):
        metric_name = match.group(0)
        if has_zero_fallback:
            errors.append(
                f"{metric_name}: external metric query must not add a zero fallback"
            )
        if contract is None or metric_name not in contract.metrics:
            errors.append(f"unverified external metric family `{metric_name}`")
            continue

        matchers, parse_error = _external_selector(expr, match.end())
        if parse_error is not None:
            errors.append(f"{metric_name}: {parse_error}")
            continue
        spec = contract.metrics[metric_name]
        assert matchers is not None
        for label, expected_value in spec.required_matchers.items():
            actual_matcher = matchers.get(label)
            if actual_matcher is None:
                errors.append(f"{metric_name}: missing required matcher `{label}`")
            elif (
                actual_matcher[0] == "="
                and actual_matcher[1] != expected_value
                and actual_matcher[1] in spec.allowed_matchers.get(label, frozenset())
            ):
                errors.append(
                    f'{metric_name}: required matcher `{label}` must equal '
                    f'"{expected_value}"'
                )
        for label, (operator, value) in matchers.items():
            if label in IDENTIFYING_EXTERNAL_LABELS:
                errors.append(
                    f"{metric_name}: identifying selector label `{label}` is forbidden"
                )
                continue
            if operator != "=":
                errors.append(
                    f"{metric_name}: external selector requires exact equality"
                )
            allowed_values = spec.allowed_matchers.get(label)
            if allowed_values is None:
                errors.append(f"{metric_name}: unverified selector label `{label}`")
            elif value not in allowed_values:
                errors.append(
                    f"{metric_name}: unverified selector value for `{label}`"
                )
    return errors


def _panel_targets(panel: dict):
    if isinstance(panel.get("panels"), list):
        for child in panel["panels"]:
            yield from _panel_targets(child)
    else:
        yield from panel.get("targets", [])


def validate_dashboard_external_metrics(
    document: dict, contract: ExternalMetricContract | None
) -> list[str]:
    errors = []
    for panel in document.get("panels", []):
        for target in _panel_targets(panel):
            expr = target.get("expr", "")
            errors.extend(validate_external_expression(expr, contract))
            metric_names = {
                match.group(0)
                for match in EXTERNAL_METRIC_NAME.finditer(strip_strings(expr))
            }
            if not metric_names:
                continue

            legend_format = target.get("legendFormat", "")
            for template in re.findall(r"\{\{(.*?)\}\}", legend_format):
                label_match = LEGEND_LABEL.fullmatch(template)
                if label_match is None:
                    errors.extend(
                        f"{name}: unsupported external legend template"
                        for name in sorted(metric_names)
                    )
                    continue
                label = label_match.group(1)
                for name in sorted(metric_names):
                    if label in IDENTIFYING_EXTERNAL_LABELS:
                        errors.append(
                            f"{name}: identifying legend label `{label}` is forbidden"
                        )
                    elif (
                        contract is None
                        or name not in contract.metrics
                        or label not in contract.metrics[name].allowed_matchers
                    ):
                        errors.append(f"{name}: unverified legend label `{label}`")
    return errors


def check_ref_ids(path: pathlib.Path, panel: dict, failures: list[str]) -> None:
    """Every query on a panel needs its own RefId, rows included.

    Grafana keys query results, query status and per-query transformations by
    RefId, so two queries called `A` collide: the query editor reports on one
    of them and a query inspector cannot tell them apart. The generator's
    `target()` defaults to `A`, which is how a three-query panel shipped three
    queries named `A`.
    """
    if isinstance(panel.get("panels"), list):
        for child in panel["panels"]:
            check_ref_ids(path, child, failures)
        return
    ref_ids = [target.get("refId") for target in panel.get("targets", [])]
    shared = sorted({ref_id for ref_id in ref_ids if ref_ids.count(ref_id) > 1})
    if shared:
        failures.append(
            f"{path.name} / {panel.get('title', '<untitled>')}: queries share "
            f"RefId {shared} ({ref_ids})"
        )


def check_layout(path: pathlib.Path, document: dict, failures: list[str]) -> None:
    """Rows and their panels must fit the dashboard's shared grid.

    Collapsed rows own a local grid and take one top-level grid line. An
    expanded row's children use absolute dashboard coordinates, so the next
    row starts after the last child rather than one line after the header.
    Only the first row may be expanded; this keeps the overview visible while
    preserving the existing collapsed-section behavior below it.
    """
    top = document["panels"]
    non_rows = [panel for panel in top if panel["type"] != "row"]
    if non_rows:
        failures.append(
            f"{path.name}: {len(non_rows)} panel(s) sit at the top level rather "
            f"than inside a row; keep panels grouped by their row"
        )
    if path.name == "product.json" and (
        not top or top[0]["type"] != "row" or top[0].get("collapsed", True)
    ):
        failures.append(
            "product.json: the first overview row must be expanded so the "
            "product summary is visible when the dashboard opens"
        )

    positions: list[tuple[int, int]] = []
    next_y = 0
    for index, panel in enumerate(top):
        if panel["type"] != "row":
            continue
        grid = panel["gridPos"]
        collapsed = panel.get("collapsed", True)
        if not collapsed and index != 0:
            failures.append(
                f"{path.name} / {panel['title']}: only the first overview row "
                f"may be expanded"
            )
        if grid["y"] != next_y:
            failures.append(
                f"{path.name} / {panel['title']}: row y={grid['y']}; expected "
                f"y={next_y} after the preceding row's occupied grid"
            )
        if grid["x"] + grid["w"] > 24:
            failures.append(
                f"{path.name} / {panel['title']}: x+w={grid['x'] + grid['w']} "
                f"exceeds the 24-column grid"
            )
        positions.append((grid["y"], grid["x"]))
        expanded = not collapsed
        check_row_contents(path, panel, failures, expanded=expanded)

        row_bottom = grid["y"] + grid["h"]
        if expanded:
            child_bottoms = [
                child["gridPos"]["y"] + child["gridPos"]["h"]
                for child in panel.get("panels", [])
                if child["type"] != "row"
            ]
            next_y = max([row_bottom, *child_bottoms])
        else:
            next_y = row_bottom

    if positions != sorted(positions):
        failures.append(f"{path.name}: rows are not in ascending y order")


def check_row_contents(
    path: pathlib.Path,
    row: dict,
    failures: list[str],
    *,
    expanded: bool = False,
) -> None:
    """Check row children for bounds and collisions.

    Collapsed rows use a local coordinate space and must stay on one band;
    Grafana renders a multi-band collapsed row as a diagonal staircase. The
    expanded overview uses the shared dashboard grid and may have multiple
    bands, but its children must start below the row header. In either mode,
    nested rows and panel cells are still checked recursively.
    """
    children = [child for child in row.get("panels", []) if child["type"] != "row"]
    bands = sorted({child["gridPos"]["y"] for child in children})
    if not expanded and len(bands) > 1:
        failures.append(
            f"{path.name} / {row['title']}: children sit on {len(bands)} grid "
            f"bands (y={bands}); Grafana renders a multi-band collapsed row as "
            f"a diagonal staircase — give the row a single band, or split it "
            f"into one row per band"
        )

    row_bottom = row["gridPos"]["y"] + row["gridPos"]["h"]
    occupied: dict[tuple[int, int], str] = {}
    for child in row.get("panels", []):
        child_grid = child["gridPos"]
        if child["type"] == "row":
            if expanded:
                failures.append(
                    f"{path.name} / {row['title']}: expanded rows cannot contain "
                    f"nested rows"
                )
            check_row_contents(path, child, failures)
            continue
        if child_grid["x"] + child_grid["w"] > 24:
            failures.append(
                f"{path.name} / {row['title']} / {child['title']}: "
                f"x+w={child_grid['x'] + child_grid['w']} exceeds the "
                f"24-column grid"
            )
        if expanded and child_grid["y"] < row_bottom:
            failures.append(
                f"{path.name} / {row['title']} / {child['title']}: y="
                f"{child_grid['y']} overlaps the expanded row header ending "
                f"at y={row_bottom}"
            )
        # Expanded children use absolute dashboard coordinates; collapsed-row
        # children use the row's local coordinate space. In both cases a cell is
        # occupied by one panel at most.
        for row_offset in range(child_grid["h"]):
            for column in range(child_grid["x"], child_grid["x"] + child_grid["w"]):
                cell = (child_grid["y"] + row_offset, column)
                if cell in occupied:
                    failures.append(
                        f"{path.name} / {row['title']}: "
                        f"{child['title']!r} overlaps {occupied[cell]!r} "
                        f"at x={column} y={cell[0]}"
                    )
                    break
                occupied[cell] = child["title"]


# Families the crate records and deliberately does not chart, each with the
# reason. A list rather than a heuristic: every entry is a decision somebody
# made, and it has to be re-made when the family changes.
_STDIO_ONLY = (
    "recorded by the stdio-only filesystem watcher; the artifacts in this "
    "directory describe the HTTP profile, which refuses MEMORY_INGESTION_INBOX "
    "at startup and never wires a watcher, so a panel of it would be empty in "
    "every deployment it is deployed to"
)
UNREAD_BY_DECISION = {
    # Filesystem ingestion is the stdio-only path. `MEMORY_INGESTION_INBOX` is
    # refused at startup in the HTTP profile, and the HTTP binary never wires a
    # watcher, so a panel or rule over `memory_fs_watch_*` is empty in every
    # deployment these artifacts describe. The families stay in `DESCRIPTIONS`
    # because a stdio build still records them for an operator who brings their
    # own Prometheus — deleting the vocabulary would make the exporter describe
    # a family the code records.
    #
    # `queue_depth` carries a second reason, independent of the profile: it is set
    # once at startup from a recovery pass and never updated, so it is a snapshot
    # of what was queued when the process started. Charting it draws that snapshot
    # as if it were a backlog.
    "memory_fs_watch_revisions_total": _STDIO_ONLY,
    "memory_fs_watch_retries_total": _STDIO_ONLY,
    "memory_fs_watch_revision_duration_seconds": _STDIO_ONLY,
    "memory_fs_watch_scan_files_total": _STDIO_ONLY,
    "memory_fs_watch_inflight": _STDIO_ONLY,
    "memory_fs_watch_degraded": _STDIO_ONLY,
    "memory_fs_watch_queue_depth": (
        _STDIO_ONLY
        + " It is also set once at startup from a recovery pass and never "
        "updated again, so a panel of it would draw a snapshot as if it were a "
        "queue"
    ),
}


def families_with_no_reader(recorded: set[str], families: set[str]) -> list[str]:
    """Recorded families that no rule and no panel reads.

    A metric nobody reads is not documentation, it is cost: it is scraped on
    every request, stored, and never looked at. The failure is silent in both
    directions — the exposition carries the series and the dashboard looks
    complete — so it is worth a check rather than a reviewer's memory.

    A name that is a strict prefix of another family is not a family: `job` in a
    test's own string literal, or a prefix used to match a whole group of them.
    Those are excluded here rather than allow-listed, because a prefix that
    happens to equal a real family name is vanishingly unlikely and an
    allow-list is something nobody re-reads.
    """
    charted = _charted_names(recorded, families)
    prefix_of_a_family = {
        name for name in families
        if any(other != name and other.startswith(name) for other in families)
    }
    unread = [
        family for family in sorted(families)
        if family not in charted and family not in prefix_of_a_family
    ]
    return [
        f"{family} — {UNREAD_BY_DECISION[family]}"
        if family in UNREAD_BY_DECISION else family
        for family in unread
    ]


# Suffixes the exporter appends to a summary's series. A rule that reads
# `…_sum` or `…_count` is reading the family, and a word-boundary match on the
# bare name would call it unreadable — which is how `memory_claim_candidates_
# considered` looked like an orphan while a rule stood right there dividing its
# `_sum` by its `_count`.
DERIVED = ("_sum", "_count", "_bucket")


def _charted_names(recorded: set[str], families: set[str]) -> set[str]:
    """Every family a recording rule or a panel expression reads.

    Read as the identifiers the expressions actually contain, then mapped back
    to families through the derived suffixes, rather than matched by name: the
    bare name of a summary family does not appear in the very expressions that
    read it.
    """
    text = RULES.read_text()
    for path in sorted(DASHBOARDS.glob("*.json")):
        document = json.loads(path.read_text())
        for panel in document["panels"]:
            text += "\n" + "\n".join(panel_expressions(panel))
    mentioned = set(re.findall(r"\b(memory_[a-z0-9_]+)", strip_strings(text)))
    read = set(mentioned)
    for name in mentioned:
        for suffix in DERIVED:
            if name.endswith(suffix) and name[: -len(suffix)] in families | recorded:
                read.add(name[: -len(suffix)])
    return read


def main() -> int:
    try:
        external_contract = load_external_metric_contract()
    except (OSError, UnicodeError, ValueError, yaml.YAMLError):
        print("FAIL external metric contract is invalid or unreadable")
        return 1

    recorded = recorded_series()
    families = exported_families()
    declared = bounded_vocabulary()
    known = recorded | families

    failures: list[str] = []
    panels_checked = 0

    for path in sorted(DASHBOARDS.glob("*.json")):
        document = json.loads(path.read_text())
        for error in validate_dashboard_external_metrics(document, external_contract):
            failures.append(f"{path.name}: {error}")
        check_layout(path, document, failures)
        for panel in document["panels"]:
            check_ref_ids(path, panel, failures)
        for panel in document["panels"]:
            for expr in panel_expressions(panel):
                if not expr:
                    continue
                panels_checked += 1
                # Quoted strings are label values, not series: a panel that
                # filters `up{job="memory_mcp"}` must not be read as naming a
                # metric called `memory_mcp`.
                scanned = strip_strings(expr)
                for name in re.findall(r"\b(memory:[a-z0-9_:]+)", scanned):
                    if name not in known:
                        title = panel.get("title", "<untitled>")
                        failures.append(
                            f"{path.name} / {title}: reads `{name}`, which is "
                            f"neither a recorded series nor an exported metric"
                        )
                for metric in re.findall(r"\b(memory_[a-z0-9_]+)", scanned):
                    base = metric
                    for suffix in ("_sum", "_count"):
                        if metric.endswith(suffix) and metric[: -len(suffix)] in known:
                            base = metric[: -len(suffix)]
                            break
                    if base not in known:
                        title = panel.get("title", "<untitled>")
                        failures.append(
                            f"{path.name} / {title}: reads `{metric}`, which is "
                            f"neither a recorded series nor an exported metric"
                        )
                # A filter on a label value the crate cannot emit matches
                # nothing, so the panel renders empty — indistinguishable from
                # a subsystem that is off, which is the failure this whole file
                # exists to catch.
                for value in unknown_label_values(expr, declared):
                    title = panel.get("title", "<untitled>")
                    failures.append(
                        f"{path.name} / {title}: filters on `{value}`, which "
                        f"the crate cannot emit, so this panel can only render "
                        f"empty"
                    )

    for orphan in families_with_no_reader(recorded, families):
        family = orphan.split(" — ")[0]
        if family in UNREAD_BY_DECISION:
            continue
        failures.append(
            f"the crate exports `{family}` and nothing reads it: no recording "
            f"rule, no panel. A metric nobody reads is scraped and stored "
            f"forever without informing anyone — either chart it, or record it "
            f"in UNREAD_BY_DECISION with the reason it is left unread"
        )
    print(f"{len(recorded)} recorded series, {len(families)} exported families")
    print(f"{panels_checked} panel expressions checked")
    for failure in failures:
        print(f"  FAIL {failure}")
    if failures:
        return 1
    print(
        "  every panel reads a series that exists, rows fit the dashboard grid, "
        "collapsed rows use one band, and every query has its own RefId"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
