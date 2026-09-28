"""Every checkable claim in the evidence docs must resolve against the tree.

The same defect class has now been caught twice by review: a number or a
`file.rs:NNN` citation written from memory instead of measured, in the
documents that are the migration's evidence. This makes that class fail
the build instead of surviving until someone re-reads the prose.

It covers three things:
  * cited files exist, and are cited unambiguously (three files are named
    `logging.rs`, so `src/logging.rs` is required, not `logging.rs`);
  * cited line numbers land on a line with content;
  * cited identifiers exist somewhere in the tree — as a Rust function or
    test, a Python test, a cargo target, or a test/bench file stem.
"""
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))


class DocClaims(unittest.TestCase):
    def test_every_cited_file_line_and_identifier_resolves(self):
        result = subprocess.run(
            [sys.executable, os.path.join(HERE, "audit_doc_claims.py")],
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
        self.assertEqual(
            result.returncode,
            0,
            "a claim in the evidence docs does not resolve against the tree:\n"
            f"{result.stdout}{result.stderr}",
        )

    def test_the_audit_would_notice_a_bad_citation(self):
        """The auditor itself must fail on a claim it should catch.

        Without this, a refactor that made it always report success would
        leave the assertion above green while checking nothing.

        The probe is a temporary `CONTEXT.md` at the repository root. It used
        to write into `docs/architecture`, which was removed with the migration
        record, so the probe could not be created and the test failed before it
        asserted anything — exactly the "passes empty directories" failure mode
        the auditor's own spec warns about.

        `CONTEXT.md` is chosen over `docs/operations` because the auditor
        exempts the operator runbooks from identifier checks: they name CLI
        commands and target architectures, so symbol existence is not a
        meaningful test there. A root-level `CONTEXT.md` gets the full check,
        which is what this test needs to exercise.
        """
        probe = os.path.join(ROOT, "CONTEXT.md")
        original = None
        if os.path.exists(probe):
            with open(probe, encoding="utf-8") as handle:
                original = handle.read()
        try:
            with open(probe, "w", encoding="utf-8") as handle:
                handle.write(
                    "claims `nonexistent_module.rs:1`, `not_a_real_symbol_xq`, "
                    "and `shared/ids.rs:0`.\n"
                )
            result = subprocess.run(
                [sys.executable, os.path.join(HERE, "audit_doc_claims.py")],
                capture_output=True,
                text=True,
                cwd=ROOT,
            )
            self.assertNotEqual(
                result.returncode,
                0,
                "the claim auditor accepted every planted defect",
            )
            output = result.stdout + result.stderr
            for needle in (
                "nonexistent_module.rs",
                "not_a_real_symbol_xq",
                "non-positive line number",
            ):
                self.assertIn(
                    needle,
                    output,
                    f"the claim auditor did not report the planted defect: {needle}",
                )
        finally:
            if original is not None:
                with open(probe, "w", encoding="utf-8") as handle:
                    handle.write(original)


if __name__ == "__main__":
    unittest.main()
