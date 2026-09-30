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
from check_rules import exported_families  # noqa: E402


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


def main() -> int:
    recorded = recorded_series()
    families = exported_families()
    known = recorded | families

    failures: list[str] = []
    panels_checked = 0

    for path in sorted(DASHBOARDS.glob("*.json")):
        document = json.loads(path.read_text())
        for panel in document["panels"]:
            for expr in panel_expressions(panel):
                if not expr:
                    continue
                panels_checked += 1
                for name in re.findall(r"\b(memory:[a-z0-9_:]+)", expr):
                    if name not in known:
                        title = panel.get("title", "<untitled>")
                        failures.append(
                            f"{path.name} / {title}: reads `{name}`, which is "
                            f"neither a recorded series nor an exported metric"
                        )

    print(f"{len(recorded)} recorded series, {len(families)} exported families")
    print(f"{panels_checked} panel expressions checked")
    for failure in failures:
        print(f"  FAIL {failure}")
    if failures:
        return 1
    print("  every panel reads a series that exists")
    return 0


if __name__ == "__main__":
    sys.exit(main())
