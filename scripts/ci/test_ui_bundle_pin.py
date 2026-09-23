"""Regression checks for the pinned Dioxus CLI and the bundle it must produce.

`Dockerfile` is the only place the console's bundle is built, and it pins the CLI
in one `ARG`. Nothing else in the repository records which CLI version was
verified, so these checks are what keeps the pin from drifting away from the
crate's own `dioxus` requirement, and what keeps the output layout the build
stage asserts from being edited into something `dx` no longer produces.

The layout checks are textual on purpose: they pin the contract between the
Dockerfile's `dx bundle` invocation, its `MEMORY_MCP_CONTROL_PLANE_UI_DIST`, and
the assertions that follow it, without running a container. Whitespace is
normalised before matching, so reindenting the build block does not fail a test
that is about the commands in it.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCKERFILE = ROOT / "Dockerfile"
UI_MANIFEST = ROOT / "crates" / "control-plane-ui" / "Cargo.toml"


def collapsed(text: str) -> str:
    """One line, single spaces: the commands, not their formatting."""
    return " ".join(text.split())


class DioxusCliPinTests(unittest.TestCase):
    def setUp(self):
        self.raw = DOCKERFILE.read_text(encoding="utf-8")
        self.text = collapsed(self.raw)

    def test_the_cli_pin_matches_the_dioxus_dependency(self):
        # The CLI compiles the crate against the framework it carries, so a CLI
        # that is older or newer than the crate's own `dioxus` requirement builds
        # something the verified bundle was not verified as.
        pinned = re.search(r"^ARG DIOXUS_CLI_VERSION=(\S+)$", self.raw, re.MULTILINE)
        self.assertIsNotNone(pinned, "the Dockerfile no longer pins DIOXUS_CLI_VERSION")
        requirement = re.search(
            r'^dioxus\s*=\s*\{\s*version\s*=\s*"([^"]+)"',
            UI_MANIFEST.read_text(encoding="utf-8"),
            re.MULTILINE,
        )
        self.assertIsNotNone(requirement, "the UI crate no longer declares dioxus = { version = ... }")
        self.assertEqual(requirement.group(1), f"={pinned.group(1)}")

    def test_the_bundle_is_built_from_the_workspace_root_with_an_explicit_package(self):
        # `dx` resolves `default-members` relative to the current directory, so
        # running it from the crate directory panics on a missing path. The
        # explicit `--package` is what makes the root invocation unambiguous.
        self.assertIn("cd /src;", self.text)
        self.assertIn("dx bundle --platform web --release --package control-plane-ui", self.text)

    def test_the_dist_directory_is_the_one_the_build_script_reads(self):
        # `dx bundle --out-dir X` writes the document to `X/public`, and
        # `crates/memory-mcp/build.rs` requires a non-empty `index.html` at the
        # root of whatever `MEMORY_MCP_CONTROL_PLANE_UI_DIST` names.
        self.assertIn("--out-dir /src/control-plane-ui-dist", self.text)
        self.assertIn(
            "ENV MEMORY_MCP_CONTROL_PLANE_UI_DIST=/src/control-plane-ui-dist/public",
            self.text,
        )

    def test_the_build_asserts_the_layout_it_depends_on(self):
        # One document, one stylesheet, one module and one WebAssembly payload:
        # `crates/memory-mcp/build.rs` embeds an index plus the assets it
        # references, and a stale staging directory would add a second pair.
        self.assertIn("test -s /src/control-plane-ui-dist/public/index.html;", self.text)
        for extension in ("js", "wasm", "css"):
            self.assertIn(
                f"find /src/control-plane-ui-dist/public -type f -name '*.{extension}' "
                '| wc -l)" = "1"',
                self.text,
            )

    def test_base_path_plumbing(self):
        # The UI bundle bakes its mount prefix at build time; the runtime
        # twin is the path of MEMORY_MCP_HTTP_PUBLIC_BASE_URL (see
        # docs/superpowers/specs/2026-09-23-path-prefix-deployment.md).
        # The arg must default to empty so root builds stay zero-config.
        self.assertIn("ARG MEMORY_MCP_UI_BASE_PATH=", self.text)
        self.assertIn("--base-path", self.text)
        self.assertIn("${MEMORY_MCP_UI_BASE_PATH}", self.text)

    def test_the_staging_directory_is_cleared_before_bundling(self):
        # `dx` copies its staging directory wholesale, so without this the
        # embedded bundle grows by one stale JS/WASM pair per build.
        self.assertIn(
            "rm -rf /src/target/dx/control-plane-ui/release/web/public /src/control-plane-ui-dist;",
            self.text,
        )


if __name__ == "__main__":
    unittest.main()
