#!/usr/bin/env python3
"""Assert that the packaged single binary serves its whole UI from memory.

The control-plane SPA is embedded into `memory_mcp_http` at compile time
(`crates/memory-mcp/build.rs` stages the Dioxus bundle and emits
`include_bytes!` for every file), so a running image must be able to serve
`/`, its JavaScript, its WebAssembly module, its stylesheet and its icon
without any file on disk, any sidecar process and any third-party host.

This script is the CI check for that property. It is deliberately stdlib-only
and browser-free so it can run next to the Compose smoke test: it fetches the
served document, resolves every asset the document references exactly the way a
browser would, and requires each one to come back from the binary with a
plausible content type. It also re-asserts the serving policy that the
crates/memory-mcp unit tests cover, because those run against a test fixture
rather than the shipped image, and that the Host allowlist actually covers the
UI surface rather than only the routes that happen to be registered before it.

Usage:
    python3 scripts/ci/assert_embedded_ui.py --base-url http://localhost:8080

    # A loopback socket has no allowlisted name of its own: the host middleware
    # compares the raw `Host` header (port included), so the probe must present
    # one that `ALLOWED_HOSTS` lists.
    python3 scripts/ci/assert_embedded_ui.py \
        --base-url http://127.0.0.1:8080 --host-header localhost

Exit status is 0 only when every check passed and the number of checks is
non-zero; it never skips.
"""

from __future__ import annotations

import argparse
import posixpath
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from html.parser import HTMLParser

# Content types we are willing to accept per asset extension. A browser refuses
# to execute a module script or instantiate WebAssembly served under the wrong
# type, so this is a correctness check, not cosmetics.
EXPECTED_CONTENT_TYPES = {
    ".js": ("javascript", "ecmascript"),
    ".mjs": ("javascript", "ecmascript"),
    ".wasm": ("application/wasm",),
    ".css": ("text/css",),
    ".svg": ("image/svg+xml",),
    ".ico": ("image/",),
    ".png": ("image/png",),
    ".woff2": ("font/woff2",),
}

# `rel` values whose `href` the browser fetches without being asked to.
FETCHED_LINK_RELS = frozenset(
    {"stylesheet", "preload", "modulepreload", "icon", "apple-touch-icon", "manifest"}
)

# The Dioxus CLI's default title, which must never reach production.
FRAMEWORK_DEFAULT_TITLE = re.compile(r"dioxus|\N{TENT}", re.IGNORECASE)

# A `Host` no allowlist can contain: `.invalid` is reserved by RFC 2606 and is
# never resolvable, so a deployment that answers it has no host check at all.
HOST_ALLOWLIST_SENTINEL = "ci-embedded-ui-check.invalid"

# The loader fetches its WebAssembly module and its JavaScript by path from
# inside the module it has already loaded, so those are not reachable from the
# HTML. Scanning the served JavaScript is what makes "the whole UI is embedded"
# checkable rather than only the HTML's half of it.
SCRIPT_ASSET_REFERENCE = re.compile(r"""["'](/[^"'\s]*\.(?:wasm|js|mjs))["']""")

# A bound on how much of the bundle one run will fetch.
MAX_ASSETS = 64

IMMUTABLE_CACHE_CONTROL = "public, max-age=31536000, immutable"
REVALIDATE_CACHE_CONTROL = "no-cache"


def header_value(headers: dict[str, str], name: str) -> str:
    """Read an HTTP header without depending on the server's casing."""
    name = name.lower()
    return next((value for key, value in headers.items() if key.lower() == name), "")


def is_content_addressed(path: str) -> bool:
    """Match the conservative filename rule used by the Rust build script."""
    filename = posixpath.basename(path)
    if filename in {"index.html", "favicon.svg"}:
        return False
    stem, _ = posixpath.splitext(filename)
    _, separator, suffix = stem.rpartition("-")
    return bool(separator and len(suffix) >= 8 and re.fullmatch(r"[a-z0-9]+", suffix))


def asset_references_in_script(source: str) -> list[str]:
    """Same-origin bundle paths a served script fetches at runtime."""
    return list(dict.fromkeys(SCRIPT_ASSET_REFERENCE.findall(source)))


class AssetReferences(HTMLParser):
    """Collect the asset URLs a served document asks the browser to fetch."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.references: list[str] = []
        self.title: str | None = None
        self.title_count = 0
        self.lang: str | None = None
        self.descriptions: list[str] = []
        self.color_schemes: list[str] = []
        self.noscript: list[str] = []
        self.ids: set[str] = set()
        self._in_title = False
        self._in_noscript = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = {name.lower(): (value or "") for name, value in attrs}

        if attributes.get("id"):
            self.ids.add(attributes["id"])

        if tag == "html":
            self.lang = attributes.get("lang")
        elif tag == "title":
            self._in_title = True
            self.title_count += 1
            if self.title is None:
                self.title = ""
        elif tag == "noscript":
            self._in_noscript = True
        elif tag == "meta":
            name = attributes.get("name", "").lower()
            content = attributes.get("content", "")
            if name == "description":
                self.descriptions.append(content)
            elif name == "color-scheme":
                self.color_schemes.append(content)
        elif tag == "script":
            source = attributes.get("src", "")
            if source:
                self.references.append(source)
        elif tag == "link":
            rels = {value.lower() for value in attributes.get("rel", "").split()}
            href = attributes.get("href", "")
            if href and rels & FETCHED_LINK_RELS:
                self.references.append(href)

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self._in_title = False
        elif tag == "noscript":
            self._in_noscript = False

    def handle_data(self, data: str) -> None:
        if self._in_title and self.title is not None:
            self.title += data
        if self._in_noscript:
            self.noscript.append(data)


def resolve_url(base_url: str, reference: str) -> str | None:
    """Resolve one reference the way a browser resolves it.

    Returns None when the reference points at another origin, which is what the
    shipped CSP (`default-src 'self'`) forbids anyway.
    """
    resolved = urllib.parse.urljoin(base_url, reference)
    parts = urllib.parse.urlsplit(resolved)
    if parts.scheme not in ("http", "https"):
        return None
    if parts.netloc != urllib.parse.urlsplit(base_url).netloc:
        return None
    # Collapse `.` and `..` segments: the bundle's own links are written as
    # `/./assets/...`, which a browser normalises before it requests.
    normalised = posixpath.normpath(parts.path)
    if parts.path.endswith("/") and not normalised.endswith("/"):
        normalised += "/"
    return urllib.parse.urlunsplit((parts.scheme, parts.netloc, normalised, parts.query, ""))


def expected_content_types(path: str) -> tuple[str, ...]:
    extension = posixpath.splitext(path)[1].lower()
    return EXPECTED_CONTENT_TYPES.get(extension, ())


def title_is_acceptable(title: str) -> bool:
    """A production document needs a real, human-authored title.

    The Dioxus default (`dioxus | <tent>`) is neither descriptive nor
    emoji-free, so it is rejected outright.
    """
    stripped = title.strip()
    if not stripped:
        return False
    if FRAMEWORK_DEFAULT_TITLE.search(stripped):
        return False
    return stripped.isascii()


def request_headers(host_header: str | None) -> dict[str, str]:
    """Headers for one probe request.

    Setting `Host` is not optional when the probe is addressed to a loopback
    socket: the host middleware compares the raw header against `ALLOWED_HOSTS`
    with no port stripping, so `http://127.0.0.1:8080` arrives as
    `Host: 127.0.0.1:8080` and is refused with `403` unless it is rewritten to a
    name the deployment lists. urllib honours an explicit `Host`, so the
    rewrite is reliable.
    """
    headers = {"Accept": "*/*"}
    if host_header:
        headers["Host"] = host_header
    return headers


class Probe:
    """Accumulate check results and HTTP responses for one run."""

    def __init__(self, base_url: str, host_header: str | None = None) -> None:
        self.base_url = base_url
        self.host_header = host_header
        self.passed = 0
        self.failures: list[str] = []

    def check(self, label: str, condition: bool, detail: object = "") -> None:
        if condition:
            self.passed += 1
            print(f"  ok   {label}")
        else:
            self.failures.append(label)
            print(f"  FAIL {label}: {detail}")

    def get(
        self,
        url: str,
        *,
        timeout: int = 15,
        host_header: str | None = None,
    ) -> tuple[int, dict[str, str], bytes]:
        host = self.host_header if host_header is None else host_header
        request = urllib.request.Request(url, headers=request_headers(host))
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.status, dict(response.headers), response.read()
        except urllib.error.HTTPError as error:
            return error.code, dict(error.headers or {}), error.read()
        except urllib.error.URLError as error:
            raise SystemExit(f"assert_embedded_ui: cannot reach {url}: {error}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--base-url",
        default="http://localhost:8080",
        help="origin of the running memory_mcp_http image",
    )
    parser.add_argument(
        "--host-header",
        default=None,
        help=(
            "send this `Host` verbatim on every request; required when the "
            "origin is a loopback socket that `ALLOWED_HOSTS` does not list "
            "with its port"
        ),
    )
    args = parser.parse_args()
    base_url = args.base_url.rstrip("/") + "/"

    probe = Probe(base_url, args.host_header)
    print(f"assert_embedded_ui: base={args.base_url} host={args.host_header or '(default)'}")

    status, headers, body = probe.get(base_url)
    probe.check(
        "document is served",
        status == 200,
        f"{status} for Host {args.host_header}"
        if args.host_header
        else (
            f"{status}; a `403` here means the request's Host is not on "
            "ALLOWED_HOSTS, which compares the header verbatim, port included "
            "(see --host-header)"
        ),
    )
    content_type = header_value(headers, "content-type")
    probe.check("document is html", "text/html" in content_type, content_type)
    probe.check(
        "document is revalidated on deploy",
        header_value(headers, "cache-control") == REVALIDATE_CACHE_CONTROL,
        header_value(headers, "cache-control"),
    )

    fallback_url = urllib.parse.urljoin(base_url, "admin/login")
    fallback_status, fallback_headers, _ = probe.get(fallback_url)
    probe.check("SPA route returns the embedded document", fallback_status == 200, fallback_status)
    probe.check(
        "SPA route returns html",
        "text/html" in header_value(fallback_headers, "content-type"),
        header_value(fallback_headers, "content-type"),
    )
    probe.check(
        "SPA route is revalidated on deploy",
        header_value(fallback_headers, "cache-control") == REVALIDATE_CACHE_CONTROL,
        header_value(fallback_headers, "cache-control"),
    )

    # The serving policy travels with every response; re-assert it here because
    # the crate's unit test runs against a fixture, not the shipped image.
    policy = headers.get("content-security-policy", headers.get("Content-Security-Policy", ""))
    script_src = next(
        (part.strip() for part in policy.split(";") if part.strip().startswith("script-src")),
        "",
    )
    probe.check("csp is present", bool(policy), policy)
    probe.check("csp allows wasm compilation", "'wasm-unsafe-eval'" in script_src, script_src)
    probe.check("csp refuses general eval", not re.search(r"(^|\s)'unsafe-eval'", policy))
    probe.check("csp refuses inline script", not re.search(r"(^|\s)'unsafe-inline'", policy))
    probe.check(
        "nosniff is set",
        headers.get("x-content-type-options", headers.get("X-Content-Type-Options", ""))
        == "nosniff",
    )
    probe.check(
        "referrer policy is set",
        headers.get("referrer-policy", headers.get("Referrer-Policy", "")) == "no-referrer",
    )

    # The Host allowlist is a property of the deployment boundary (spec §3.3),
    # not of a route: `/` and `/assets/*` have no route of their own, so a check
    # installed before the fallback exists is escaped by exactly the surface this
    # script is about. Assert it on the shipped image, where it is observable.
    refused_status, _, refused_body = probe.get(
        base_url, host_header=HOST_ALLOWLIST_SENTINEL
    )
    probe.check(
        "an unlisted Host is refused for the UI",
        refused_status == 403,
        f"{refused_status} for Host {HOST_ALLOWLIST_SENTINEL}",
    )
    probe.check(
        "the refusal does not return the document",
        refused_body != body,
        f"{len(refused_body)} bytes",
    )

    document = AssetReferences()
    document.feed(body.decode("utf-8", errors="replace"))

    # A document whose shell is wrong is not a working console even when every
    # asset loads, so the shell is checked here too.
    probe.check("document declares a language", bool(document.lang), document.lang)
    # Two title elements means the shell and the bundler both wrote one, which
    # is what duplicated the configured title before.
    probe.check("document has exactly one title", document.title_count == 1, document.title_count)
    probe.check(
        "document title is the product",
        title_is_acceptable(document.title or ""),
        repr(document.title),
    )
    probe.check(
        "document has a description",
        any(description.strip() for description in document.descriptions),
    )
    probe.check(
        "document declares the dark theme",
        any("dark" in value for value in document.color_schemes),
        document.color_schemes,
    )
    probe.check(
        "document explains a javascript-disabled client",
        any(fragment.strip() for fragment in document.noscript),
    )
    # The `<main>` landmark is added by the app once it mounts, so it cannot be
    # asserted against the served shell; the browser scenario covers it. What
    # the shell must provide is the mount point the app renders into.
    probe.check("document provides the app mount point", "main" in document.ids, sorted(document.ids))

    probe.check("document references assets", bool(document.references), len(document.references))

    served: dict[str, tuple[int, str, str]] = {}
    external: list[str] = []
    unresolved: list[str] = []
    pending = list(document.references)

    while pending:
        reference = pending.pop(0)
        resolved = resolve_url(base_url, reference)
        if resolved is None:
            external.append(reference)
            continue
        path = urllib.parse.urlsplit(resolved).path
        if path in served:
            continue
        if len(served) >= MAX_ASSETS:
            unresolved.append(f"more than {MAX_ASSETS} assets referenced")
            break
        asset_status, asset_headers, asset_body = probe.get(resolved)
        asset_type = header_value(asset_headers, "content-type")
        cache_control = header_value(asset_headers, "cache-control")
        served[path] = (asset_status, asset_type, cache_control)
        if asset_status != 200:
            unresolved.append(f"{path} -> {asset_status}")
            continue
        if not asset_body:
            unresolved.append(f"{path} -> empty body")
            continue
        wanted = expected_content_types(path)
        if wanted and not any(candidate in asset_type for candidate in wanted):
            unresolved.append(f"{path} -> content-type {asset_type!r}")
            continue
        probe.check(f"{path} has a cache policy", bool(cache_control), cache_control)
        if is_content_addressed(path):
            probe.check(
                f"{path} is immutable",
                cache_control == IMMUTABLE_CACHE_CONTROL,
                cache_control,
            )
        if posixpath.basename(path) == "favicon.svg":
            probe.check(
                "favicon is revalidated on deploy",
                cache_control == REVALIDATE_CACHE_CONTROL,
                cache_control,
            )
        if path.endswith((".js", ".mjs")):
            pending.extend(
                asset_references_in_script(asset_body.decode("utf-8", errors="replace"))
            )

    probe.check("no asset is fetched from another origin", not external, external)
    probe.check(
        "every referenced asset is served from the binary",
        not unresolved,
        unresolved,
    )
    print(f"assert_embedded_ui: served {sorted(served)}")

    # A bundle that lost its stylesheet still boots, so require the kinds of
    # asset the console actually needs rather than only "some" asset.
    kinds = {posixpath.splitext(path)[1].lower() for path in served}
    for required, label in (
        (".js", "javascript"),
        (".wasm", "webassembly"),
        (".css", "stylesheet"),
        (".svg", "icon"),
    ):
        probe.check(f"bundle includes a {label}", required in kinds, sorted(kinds))

    if probe.failures:
        print(
            f"assert_embedded_ui: FAILED ({len(probe.failures)} of "
            f"{probe.passed + len(probe.failures)} checks): {', '.join(probe.failures)}",
            file=sys.stderr,
        )
        return 1

    print(
        f"assert_embedded_ui: OK ({probe.passed} checks, "
        f"{len(served)} assets served from the binary)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
