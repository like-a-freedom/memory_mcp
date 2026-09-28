"""No source file may sit in the crate undeclared, and the audit must catch it.

An undeclared `.rs` file is the one defect class in this tree that no compiler
check reports. rustc never reads such a file, so it cannot emit a `dead_code`
warning or a clippy lint; no test executes it; and the bounded-context
migration left 137 of them behind, unnoticed, because the build stayed green
the whole time.

Two tests, and the second is the one that matters:

  * the tree as it stands is clean;
  * the auditor actually fails when an undeclared file appears.

Without the second test, a refactor that made the auditor always report
success would leave the first green while checking nothing — the same
"passes empty directories" failure mode `test_doc_claims.py` documents for its
own probe.
"""
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
AUDIT = os.path.join(HERE, "audit_undeclared_sources.py")
PROBE = os.path.join(ROOT, "crates", "memory-mcp", "src", "undeclared_probe.rs")


def run_audit():
    return subprocess.run(
        [sys.executable, AUDIT],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )


class UndeclaredSources(unittest.TestCase):
    def test_no_source_file_is_left_undeclared(self):
        result = run_audit()
        self.assertEqual(
            result.returncode,
            0,
            "a source file is on disk but never compiled:\n"
            f"{result.stdout}{result.stderr}",
        )

    def test_the_audit_would_notice_an_undeclared_file(self):
        """The auditor must fail on the defect it exists to catch.

        The probe is a syntactically valid, entirely unused module placed in
        the crate's `src` with no `mod` declaration anywhere. It is the exact
        shape the migration left behind, so it is the shape worth proving the
        audit rejects.
        """
        if os.path.exists(PROBE):
            self.skipTest("a probe file is already present; refusing to overwrite it")
        try:
            with open(PROBE, "w", encoding="utf-8") as handle:
                handle.write("//! Probe module. Never declared; never compiled.\n")
            result = run_audit()
            self.assertNotEqual(
                result.returncode,
                0,
                "the auditor accepted a file that no mod declaration reaches",
            )
            output = result.stdout + result.stderr
            self.assertIn(
                "undeclared_probe.rs",
                output,
                f"the auditor failed without naming the offending file:\n{output}",
            )
            self.assertIn(
                "never compiled",
                output,
                f"the auditor failed without explaining the defect:\n{output}",
            )
        finally:
            if os.path.exists(PROBE):
                os.remove(PROBE)


if __name__ == "__main__":
    unittest.main()
