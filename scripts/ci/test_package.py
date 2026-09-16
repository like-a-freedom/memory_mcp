"""Regression checks for artifacts, runtime dependencies and failure propagation."""

import hashlib
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import package


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.temp.name)
        Path("LICENSE").write_text("license")
        self.build = Path("build").resolve()
        self.build.mkdir()
        for binary in ("memory_mcp", "memory_mcp_http"):
            (self.build / binary).write_bytes(binary.encode())

    def tearDown(self):
        os.chdir(self.previous)
        self.temp.cleanup()

    def test_archive_contains_both_binaries_and_matching_checksum(self):
        with patch.object(package, "smoke"):
            package.package(self.build, "aarch64-unknown-linux-gnu")
        archive = next(Path("dist").glob("*.tar.gz"))
        with tarfile.open(archive) as content:
            names = {Path(name).name for name in content.getnames()}
            self.assertTrue({"memory_mcp", "memory_mcp_http", "LICENSE"} <= names)
        expected = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(Path(str(archive) + ".sha256").read_text().strip(), expected)

    def test_failed_smoke_never_produces_release_assets(self):
        with patch.object(package, "smoke", side_effect=RuntimeError("MCP failed")):
            with self.assertRaisesRegex(RuntimeError, "MCP failed"):
                package.package(self.build, "aarch64-unknown-linux-gnu")
        self.assertEqual(list(Path("dist").iterdir()), [])

    def test_dynamic_library_disables_standalone_download(self):
        (self.build / "libdependency.so").write_bytes(b"library")
        with patch.object(package, "smoke"):
            package.package(self.build, "aarch64-unknown-linux-gnu")
        self.assertFalse(Path("dist/memory_mcp_linux_aarch64").exists())
        with tarfile.open(next(Path("dist").glob("*.tar.gz"))) as content:
            self.assertIn("libdependency.so", {Path(n).name for n in content.getnames()})

    def test_windows_setup_does_not_override_dependency_crt(self):
        setup = (self.previous / ".github/actions/setup/action.yml").read_text()
        for flag in ("CFLAGS=/MT", "CXXFLAGS=/MT", "CMAKE_C_FLAGS=/MT", "CMAKE_CXX_FLAGS=/MT"):
            self.assertNotIn(flag, setup)
        self.assertNotIn("crt-static", setup)
