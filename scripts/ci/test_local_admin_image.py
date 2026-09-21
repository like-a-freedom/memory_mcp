"""Pin the image harness and the browser runner to the same scenario registry.

`scripts/ci/local_admin_image.py` decides which scenarios run.
`scripts/ci/local_admin_browser.mjs` decides which names it will accept and how
each one is dispatched. They are two lists, in two languages, with no shared
source of truth, so adding a scenario to one and forgetting the other fails only
at runtime — inside a container, after a build.

That is not hypothetical: adding the `flow` scenario to the harness first made
`--scenario all` fail with

    unknown scenario 'flow'; expected one of auth, clients, regression, ui

after the stack was already up. These tests make that mismatch a unit-test
failure instead, and they run in CI without Docker, Node or a browser.

The harness also owns a second registry, `HARNESS_SCENARIOS`, for scenarios whose
subject is not a browser flow (`removal` drives the CLI and then the routes). That
registry has no JavaScript counterpart, so it is pinned here instead to an
implementation the dispatcher can actually find.
"""

import re
import unittest
from pathlib import Path

import local_admin_image as harness

HERE = Path(__file__).resolve().parent
RUNNER = HERE / "local_admin_browser.mjs"

REGISTRY_RE = re.compile(r"const SCENARIOS = \[([^\]]*)\];")
COMPARISON_RE = re.compile(r"SCENARIO === '([a-z]+)'")
DISPATCH_RE = re.compile(r"await (scenario[A-Za-z]+)\(context\)")
FUNCTION_RE = re.compile(r"async function (scenario[A-Za-z]+)\(")


class ScenarioRegistryTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = RUNNER.read_text()

    def runner_registry(self):
        match = REGISTRY_RE.search(self.source)
        self.assertIsNotNone(match, "local_admin_browser.mjs declares no SCENARIOS")
        return [name for name in re.findall(r"'([^']*)'", match.group(1))]

    def test_both_files_declare_the_same_scenarios(self):
        # The browser registry only: `HARNESS_SCENARIOS` names no JavaScript
        # scenario, so comparing it here would pin the runner to a name it must
        # reject.
        self.assertEqual(
            self.runner_registry(),
            list(harness.SCENARIOS),
            "the harness and the browser runner must accept exactly the same browser names",
        )

    def test_the_two_registries_do_not_overlap(self):
        # A name in both would be dispatched twice, once by each path, and the
        # second run would repeat mutations the first one made.
        self.assertEqual(
            set(harness.SCENARIOS) & set(harness.HARNESS_SCENARIOS),
            set(),
            "a scenario belongs to exactly one registry",
        )

    def test_every_harness_scenario_has_an_implementation(self):
        # `run_scenarios` resolves these through `getattr`, so a name without a
        # method is an AttributeError after the stack is already up.
        for name in harness.HARNESS_SCENARIOS:
            with self.subTest(name=name):
                self.assertTrue(
                    hasattr(harness.Harness, f"scenario_{name}"),
                    f"HARNESS_SCENARIOS names {name!r} but Harness has no scenario_{name}",
                )

    def test_no_harness_scenario_is_dispatched_by_the_browser_runner(self):
        for name in harness.HARNESS_SCENARIOS:
            with self.subTest(name=name):
                self.assertNotIn(
                    name,
                    self.runner_registry(),
                    "a harness-owned scenario must not also be a runner scenario",
                )

    def test_every_scenario_is_dispatched(self):
        # The runner accepts a name and then has to route it. One scenario is the
        # trailing `else` fallback and is deliberately not compared by name.
        comparisons = set(COMPARISON_RE.findall(self.source))
        fallbacks = self.source.count("else await scenario")
        expected = set(self.runner_registry())
        self.assertEqual(
            expected - comparisons,
            {"regression"},
            "every scenario except the documented fallback needs an explicit branch",
        )
        self.assertEqual(fallbacks, 1, "exactly one scenario may be the fallback")

    def test_every_compared_scenario_has_a_target_function(self):
        defined = set(FUNCTION_RE.findall(self.source))
        targets = set(DISPATCH_RE.findall(self.source))
        self.assertTrue(targets, "the runner dispatches nothing")
        self.assertEqual(
            targets - defined,
            set(),
            "a dispatched scenario has no implementation",
        )

    def test_no_dispatch_branch_references_an_unregistered_scenario(self):
        registered = set(self.runner_registry())
        compared = set(COMPARISON_RE.findall(self.source))
        self.assertEqual(
            compared - registered - {"regression"},
            set(),
            "a branch compares against a name the allowlist would reject",
        )


class ParseScenariosTest(unittest.TestCase):
    def test_all_expands_to_both_registries_in_order(self):
        self.assertEqual(harness.parse_scenarios("all"), list(harness.ALL_SCENARIOS))
        self.assertEqual(
            harness.ALL_SCENARIOS,
            harness.SCENARIOS + harness.HARNESS_SCENARIOS,
            "`all` runs the browser scenarios first and the harness-owned ones after",
        )

    def test_a_subset_is_accepted_in_the_order_given(self):
        self.assertEqual(harness.parse_scenarios("flow,auth"), ["flow", "auth"])

    def test_unknown_and_empty_selections_are_refused(self):
        import argparse

        for value in ["", "  ", "nope", "auth,nope"]:
            with self.subTest(value=value):
                with self.assertRaises(argparse.ArgumentTypeError):
                    harness.parse_scenarios(value)

    def test_every_registered_scenario_parses(self):
        for name in harness.ALL_SCENARIOS:
            with self.subTest(name=name):
                self.assertEqual(harness.parse_scenarios(name), [name])


if __name__ == "__main__":
    unittest.main()
