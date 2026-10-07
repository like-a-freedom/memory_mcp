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

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
RULES = ROOT / "observability/recording_rules.yml"
DASHBOARDS = ROOT / "observability/dashboards"

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
    """Panels must not overlap, and rows must be stacked.

    Grafana attributes a panel to a row by *grid position*, not by the JSON
    nesting, and it sorts the top level by `(y, x)`. So an expanded row whose
    panels all carry `y: 0` — which is what the generator produced before this
    check existed — imports cleanly and renders as thirty panels stacked on one
    grid cell. Nothing in Grafana reports it.

    Collapsed rows avoid the whole problem: the row takes one grid line and its
    panels are drawn from the row's own list. That is what is asserted here.
    """
    top = document["panels"]
    non_rows = [panel for panel in top if panel["type"] != "row"]
    if non_rows:
        failures.append(
            f"{path.name}: {len(non_rows)} panel(s) sit at the top level rather "
            f"than inside a row; Grafana sorts by gridPos, so an expanded row's "
            f"panels have to be positioned below it and the generator would "
            f"have to track a running y — use a collapsed row instead"
        )

    positions: list[tuple[int, int]] = []
    for index, panel in enumerate(top):
        if panel["type"] != "row":
            continue
        grid = panel["gridPos"]
        if not panel.get("collapsed"):
            failures.append(
                f"{path.name} / {panel['title']}: row is not collapsed, so its "
                f"panels are positioned on the shared grid where they can "
                f"collide with the next row's"
            )
        if grid["y"] != index:
            failures.append(
                f"{path.name} / {panel['title']}: row y={grid['y']} at position "
                f"{index}; rows must be one grid line apart in document order"
            )
        if grid["x"] + grid["w"] > 24:
            failures.append(
                f"{path.name} / {panel['title']}: x+w={grid['x'] + grid['w']} "
                f"exceeds the 24-column grid"
            )
        positions.append((grid["y"], grid["x"]))
        check_row_contents(path, panel, failures)

    if positions != sorted(positions):
        failures.append(f"{path.name}: rows are not in ascending y order")


def check_row_contents(path: pathlib.Path, row: dict, failures: list[str]) -> None:
    """No two children of a row may share a cell, and a nested row is a row.

    Grafana allows a row inside a row. The first version of this check treated
    a nested row as a panel, so its children were never visited at all: a
    layout with two overlapping panels inside a nested row reported nothing,
    and a row pushed off the right edge of the grid reported nothing. Both are
    silent — an unchecked layout is one nobody can trust.

    Children must also sit on **one** grid band. A collapsed row whose children
    span two bands is rendered by Grafana as a diagonal staircase — the
    children come out in x order, one per line, and the declared second line is
    lost — while a single-band row lays out correctly at any depth. Verified
    against Grafana 13.2.3: a two-band row at row y=0 looked fine, the same
    content at row y=9 did not, so a layout that only appears correct at the
    top of a dashboard is not correct.
    """
    children = [child for child in row.get("panels", []) if child["type"] != "row"]
    bands = sorted({child["gridPos"]["y"] for child in children})
    if len(bands) > 1:
        failures.append(
            f"{path.name} / {row['title']}: children sit on {len(bands)} grid "
            f"bands (y={bands}); Grafana renders a multi-band row as a diagonal "
            f"staircase — give the row a single band, or split it into one row "
            f"per band"
        )

    occupied: dict[tuple[int, int], str] = {}
    for child in row.get("panels", []):
        child_grid = child["gridPos"]
        if child["type"] == "row":
            check_row_contents(path, child, failures)
            continue
        if child_grid["x"] + child_grid["w"] > 24:
            failures.append(
                f"{path.name} / {row['title']} / {child['title']}: "
                f"x+w={child_grid['x'] + child_grid['w']} exceeds the "
                f"24-column grid"
            )
        # A cell is (row, column) within the section: a collapsed row is
        # drawn in its own coordinate space, so two panels collide only if they
        # share a row *and* a column.
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


def main() -> int:
    recorded = recorded_series()
    families = exported_families()
    declared = bounded_vocabulary()
    known = recorded | families

    failures: list[str] = []
    panels_checked = 0

    for path in sorted(DASHBOARDS.glob("*.json")):
        document = json.loads(path.read_text())
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

    print(f"{len(recorded)} recorded series, {len(families)} exported families")
    print(f"{panels_checked} panel expressions checked")
    for failure in failures:
        print(f"  FAIL {failure}")
    if failures:
        return 1
    print(
        "  every panel reads a series that exists, nothing overlaps, every "
        "row is one band, and every query has its own RefId"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
