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
        self.assertEqual(
            self.runner_registry(),
            list(harness.SCENARIOS),
            "the harness and the browser runner must accept exactly the same names",
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
    def test_all_expands_to_the_full_registry_in_order(self):
        self.assertEqual(harness.parse_scenarios("all"), list(harness.SCENARIOS))

    def test_a_subset_is_accepted_in_the_order_given(self):
        self.assertEqual(harness.parse_scenarios("flow,auth"), ["flow", "auth"])

    def test_unknown_and_empty_selections_are_refused(self):
        import argparse

        for value in ["", "  ", "nope", "auth,nope"]:
            with self.subTest(value=value):
                with self.assertRaises(argparse.ArgumentTypeError):
                    harness.parse_scenarios(value)

    def test_every_registered_scenario_parses(self):
        for name in harness.SCENARIOS:
            with self.subTest(name=name):
                self.assertEqual(harness.parse_scenarios(name), [name])


if __name__ == "__main__":
    unittest.main()
