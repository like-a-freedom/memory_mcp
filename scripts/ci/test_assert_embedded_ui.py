"""Regression checks for the embedded-UI assertion script.

These cover the parts that are pure logic: how a served `href`/`src` is turned
into a request URL, which content types are acceptable per extension, what
counts as an acceptable document title, what the parser extracts from a
document, and how the `Host` override is applied. The script's HTTP behaviour is
exercised by the Docker smoke test, which runs it against the real image.
"""

import http.server
import socketserver
import threading
import unittest

import assert_embedded_ui as ui


class ResolveUrlTests(unittest.TestCase):
    def test_relative_asset_is_resolved_against_the_origin(self):
        self.assertEqual(
            ui.resolve_url("http://localhost:8080/", "/assets/app.js"),
            "http://localhost:8080/assets/app.js",
        )

    def test_dot_segment_is_collapsed_like_a_browser(self):
        # The Dioxus bundle writes its own links as `/./assets/...`; a browser
        # normalises that before it requests, so the checker must too.
        self.assertEqual(
            ui.resolve_url("http://localhost:8080/", "/./assets/app-dxh1.js"),
            "http://localhost:8080/assets/app-dxh1.js",
        )

    def test_parent_segment_is_collapsed(self):
        self.assertEqual(
            ui.resolve_url("http://localhost:8080/", "/admin/../assets/app.js"),
            "http://localhost:8080/assets/app.js",
        )

    def test_query_is_preserved(self):
        self.assertEqual(
            ui.resolve_url("http://localhost:8080/", "/assets/app.js?v=2"),
            "http://localhost:8080/assets/app.js?v=2",
        )

    def test_another_origin_is_rejected(self):
        self.assertIsNone(ui.resolve_url("http://localhost:8080/", "https://cdn.example.com/a.js"))

    def test_protocol_relative_other_origin_is_rejected(self):
        self.assertIsNone(ui.resolve_url("http://localhost:8080/", "//cdn.example.com/a.js"))

    def test_non_http_scheme_is_rejected(self):
        self.assertIsNone(ui.resolve_url("http://localhost:8080/", "data:text/javascript,1"))

    def test_a_directory_reference_keeps_its_trailing_slash(self):
        self.assertEqual(
            ui.resolve_url("http://localhost:8080/", "/assets/"),
            "http://localhost:8080/assets/",
        )


class HostHeaderTests(unittest.TestCase):
    def test_no_override_by_default(self):
        self.assertNotIn("Host", ui.request_headers(None))

    def test_an_empty_override_is_ignored(self):
        self.assertNotIn("Host", ui.request_headers(""))

    def test_the_override_is_sent_verbatim(self):
        self.assertEqual(ui.request_headers("localhost")["Host"], "localhost")

    def test_urllib_honours_the_override(self):
        # The host middleware compares the raw `Host` header against
        # `ALLOWED_HOSTS` with no port stripping, so overriding it is only
        # useful if urllib does not append its own afterwards.
        received: list[str] = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802 - stdlib naming
                received.append(self.headers.get("Host", ""))
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.end_headers()
                self.wfile.write(b"ok")

            def log_message(self, *args: object) -> None:
                pass

        class Server(http.server.HTTPServer):
            def server_bind(self) -> None:
                # `HTTPServer.server_bind` resolves the FQDN, which is a
                # reverse DNS lookup that can block for tens of seconds on a
                # cold resolver. The test only needs a bound socket.
                socketserver.TCPServer.server_bind(self)
                self.server_name = "127.0.0.1"
                self.server_port = self.server_address[1]

        server = Server(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}/"
            probe = ui.Probe(url, host_header="localhost")
            status, _, _ = probe.get(url)
            # A single probe can also override the run-wide default, which is
            # what lets one run assert that an unlisted Host is refused.
            overridden, _, _ = probe.get(url, host_header=ui.HOST_ALLOWLIST_SENTINEL)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

        self.assertEqual(status, 200)
        self.assertEqual(overridden, 200)
        self.assertEqual(received, ["localhost", ui.HOST_ALLOWLIST_SENTINEL])


class ContentTypeTests(unittest.TestCase):
    def test_known_extensions_have_expectations(self):
        self.assertIn("application/wasm", ui.expected_content_types("/assets/app_bg.wasm"))
        self.assertIn("text/css", ui.expected_content_types("/assets/main-dxh1.css"))
        self.assertIn("image/svg+xml", ui.expected_content_types("/assets/favicon.svg"))
        self.assertTrue(
            any("javascript" in value for value in ui.expected_content_types("/assets/app.js")),
            "a module served as text/plain is refused by a browser",
        )

    def test_unknown_extension_is_not_constrained(self):
        self.assertEqual(ui.expected_content_types("/assets/data.bin"), ())


class CachePolicyTests(unittest.TestCase):
    def test_matches_the_rust_content_addressing_rule(self):
        self.assertTrue(ui.is_content_addressed("/assets/main-dxh8aea88cdab71b47.css"))
        self.assertTrue(ui.is_content_addressed("/assets/app-dxh395eca31249da547.js"))
        self.assertFalse(ui.is_content_addressed("/index.html"))
        self.assertFalse(ui.is_content_addressed("/assets/favicon.svg"))
        self.assertFalse(ui.is_content_addressed("/assets/app.js"))
        self.assertFalse(ui.is_content_addressed("/assets/app-short.js"))

    def test_reads_cache_control_case_insensitively(self):
        self.assertEqual(
            ui.header_value({"Cache-Control": "no-cache"}, "cache-control"),
            "no-cache",
        )
        self.assertEqual(ui.header_value({}, "cache-control"), "")


class TitleTests(unittest.TestCase):
    def test_a_real_title_is_acceptable(self):
        self.assertTrue(ui.title_is_acceptable("Memory MCP control plane"))

    def test_the_framework_default_is_rejected(self):
        self.assertFalse(ui.title_is_acceptable("dioxus | \N{TENT}"))

    def test_an_emoji_title_is_rejected(self):
        self.assertFalse(ui.title_is_acceptable("Memory MCP \N{TENT}"))

    def test_an_empty_title_is_rejected(self):
        self.assertFalse(ui.title_is_acceptable("   "))


SHELL = """<!DOCTYPE html>
<html lang="en">
  <head>
    <title>Memory MCP control plane</title>
    <meta name="description" content="Operator console.">
    <meta name="color-scheme" content="dark">
    <link rel="stylesheet" href="/./assets/main-dxh1.css" type="text/css">
    <link rel="icon" type="image/svg+xml" href="/assets/favicon.svg">
    <link rel="preload" as="script" href="/./assets/app-dxh1.js" crossorigin>
    <link rel="canonical" href="/login">
  </head>
  <body>
    <div id="main"></div>
    <noscript><p>JavaScript is required.</p></noscript>
    <main id="main-content"></main>
    <script type="module" async src="/./assets/app-dxh1.js"></script>
  </body>
</html>
"""


class ParserTests(unittest.TestCase):
    def setUp(self):
        self.document = ui.AssetReferences()
        self.document.feed(SHELL)

    def test_collects_every_asset_the_browser_fetches(self):
        self.assertEqual(
            self.document.references,
            [
                "/./assets/main-dxh1.css",
                "/assets/favicon.svg",
                "/./assets/app-dxh1.js",
                "/./assets/app-dxh1.js",
            ],
        )

    def test_ignores_a_link_the_browser_does_not_fetch(self):
        self.assertNotIn("/login", self.document.references)

    def test_reads_the_shell_metadata(self):
        self.assertEqual(self.document.lang, "en")
        self.assertEqual(self.document.title.strip(), "Memory MCP control plane")
        self.assertEqual(self.document.title_count, 1)
        self.assertEqual(self.document.descriptions, ["Operator console."])
        self.assertEqual(self.document.color_schemes, ["dark"])
        self.assertIn("main", self.document.ids)
        self.assertTrue(any("JavaScript" in part for part in self.document.noscript))

    def test_counts_a_duplicated_title(self):
        document = ui.AssetReferences()
        document.feed("<html><head><title>One</title><title>Two</title></head></html>")
        self.assertEqual(document.title_count, 2)

    def test_a_document_without_the_mount_point_is_detected(self):
        document = ui.AssetReferences()
        document.feed("<html><body><div>no mount point</div></body></html>")
        self.assertNotIn("main", document.ids)
        self.assertIsNone(document.lang)


class ScriptReferenceTests(unittest.TestCase):
    def test_finds_the_module_and_the_wasm_the_loader_fetches(self):
        # The Dioxus loader requests both of these by path at runtime, so the
        # HTML alone cannot prove they are embedded.
        source = (
            "const wasm = '/assets/control-plane-ui_bg-dxh1.wasm';"
            'import("/assets/control-plane-ui-dxh1.js");'
        )
        self.assertEqual(
            ui.asset_references_in_script(source),
            ["/assets/control-plane-ui_bg-dxh1.wasm", "/assets/control-plane-ui-dxh1.js"],
        )

    def test_deduplicates_repeated_paths(self):
        source = "'/assets/a.wasm' + '/assets/a.wasm'"
        self.assertEqual(ui.asset_references_in_script(source), ["/assets/a.wasm"])

    def test_ignores_compressed_variants_and_source_maps(self):
        # A loader probes `.br`/`.gz` and maps optionally; those are read from
        # disk by the CLI, not embedded, so they must not be required here.
        source = "'/assets/a.wasm.br' '/assets/a.wasm.gz' '/assets/a.js.map'"
        self.assertEqual(ui.asset_references_in_script(source), [])

    def test_ignores_absolute_urls(self):
        self.assertEqual(
            ui.asset_references_in_script('"https://cdn.example.com/a.js"'),
            [],
        )


if __name__ == "__main__":
    unittest.main()
