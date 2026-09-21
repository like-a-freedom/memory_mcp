#!/usr/bin/env python3
"""Local-admin Docker image acceptance harness.

    python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all

The harness is black-box: it never imports the crate. It

  * validates the Docker daemon, the image, Node, the pinned browser runner and
    OpenSSL, failing loudly when anything is missing (it never skips and never
    exits 0 without running the requested scenarios);
  * creates a disposable Docker network, a disposable SurrealDB, a stub identity
    provider, protected (0600) temp env files, and a TLS endpoint at
    https://localhost:8443 that presents a harness-generated CA;
  * invokes both binaries from the image -- the HTTP server as the container
    entrypoint and the CLI through an entrypoint override (the image is
    shell-free, so `docker exec ... sh` is never used);
  * runs the browser scenarios from scripts/ci/local_admin_browser.mjs;
  * runs the harness-owned `removal` scenario, which is a CLI and route assertion
    rather than a browser flow;
  * tears everything down in a `finally` block.

Secrets are generated per run, live only in the protected temp directory and in
process memory, and are never printed. The CLI's stdout (which carries one-time
codes) is captured in memory only; only redacted status lines reach the console.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import secrets
import shutil
import signal
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
BROWSER_RUNNER = REPO_ROOT / "scripts" / "ci" / "local_admin_browser.mjs"
BROWSER_MODULE = REPO_ROOT / "scripts" / "ci" / "node_modules" / "playwright"
CLI_IN_IMAGE = "/usr/local/bin/memory_mcp"
HTTP_ENTRYPOINT = "/usr/local/bin/memory_mcp_http"
TLS_PORT = 8443
# A stub identity provider serves the discovery document an `oidc`-enabled
# deployment fetches at startup. Published on loopback only, so the harness can
# wait for it without running a second container per probe.
OIDC_PORT = 8444
PUBLIC_BASE_URL = f"https://localhost:{TLS_PORT}"
SURREALDB_IMAGE = "surrealdb/surrealdb:v3.2.4"
CADDY_IMAGE = "caddy:2"
SCENARIOS = ("auth", "clients", "regression", "ui", "flow")
# Scenarios this harness runs itself, because the operation under test is not a
# browser flow. `removal` drives `memory_mcp admin auth-methods remove` -- which
# ADR-0057 deliberately keeps out of the console -- against the same disposable
# registry, and then restarts the server to look at the routes it left behind.
# Kept separate from `SCENARIOS` so the pin to the browser runner's allowlist
# (`test_local_admin_image.py`) stays a comparison of like with like.
HARNESS_SCENARIOS = ("removal",)
ALL_SCENARIOS = SCENARIOS + HARNESS_SCENARIOS


class HarnessError(RuntimeError):
    """A prerequisite or runtime failure the operator must resolve."""


def run(args, *, timeout=120, check=True, env=None):
    result = subprocess.run(
        args,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
        env=env,
    )
    if check and result.returncode != 0:
        raise HarnessError(
            f"command failed ({' '.join(args[:3])}...): exit {result.returncode}\n"
            f"stdout: {result.stdout[-2000:]}\nstderr: {result.stderr[-2000:]}"
        )
    return result


class Harness:
    def __init__(self, image: str, scenarios: list[str], keep: bool) -> None:
        self.image = image
        self.scenarios = scenarios
        self.keep = keep
        self.server_env: dict[str, str] = {}
        self.ca_cert: Path | None = None
        self.suffix = secrets.token_hex(4)
        self.network = f"lmcp-net-{self.suffix}"
        self.db_name = f"lmcp-db-{self.suffix}"
        self.http_name = f"lmcp-http-{self.suffix}"
        self.tls_name = f"lmcp-tls-{self.suffix}"
        self.provider_name = f"lmcp-idp-{self.suffix}"
        self.work = Path(tempfile.mkdtemp(prefix="lmcp-local-admin-"))
        self.containers: list[str] = []
        self.ready = False

    # ── prerequisites ──────────────────────────────────────────────────
    def check_prerequisites(self) -> None:
        problems: list[str] = []

        if shutil.which("docker") is None:
            problems.append("docker CLI is not on PATH")
        else:
            info = run(["docker", "info"], check=False)
            if info.returncode != 0:
                problems.append("the Docker daemon is not reachable (`docker info` failed)")

        if shutil.which("node") is None:
            problems.append("node is not on PATH (required by the browser runner)")

        if shutil.which("openssl") is None:
            problems.append("openssl is not on PATH (required to build the harness CA)")

        if not BROWSER_RUNNER.is_file():
            problems.append(f"browser runner missing: {BROWSER_RUNNER}")
        if not BROWSER_MODULE.is_dir():
            problems.append(
                "prerequisites not installed: browser runner package missing; run "
                f"`(cd {BROWSER_RUNNER.parent} && npm ci && npx playwright install --with-deps chromium)`"
            )
        else:
            syntax = run(["node", "--check", str(BROWSER_RUNNER)], check=False)
            if syntax.returncode != 0:
                problems.append(f"browser runner does not parse: {syntax.stderr.strip()}")

        if problems:
            raise HarnessError("missing prerequisites:\n  - " + "\n  - ".join(problems))

        inspect = run(["docker", "image", "inspect", self.image], check=False)
        if inspect.returncode != 0:
            raise HarnessError(
                f"image {self.image!r} not found; build it first:\n"
                f"  docker build --tag {self.image} ."
            )

        # Both binaries must exist and be executable in the image. The CLI is
        # invoked through an entrypoint override because the runtime has no shell.
        cli_probe = run(
            ["docker", "run", "--rm", "--entrypoint", CLI_IN_IMAGE, self.image, "--version"],
            check=False,
            timeout=180,
        )
        if cli_probe.returncode != 0 or "memory_mcp" not in cli_probe.stdout:
            raise HarnessError(
                f"image {self.image!r} does not run {CLI_IN_IMAGE} --version; the image must ship "
                "`memory_mcp` built with streamable-http"
            )
        # The HTTP binary has no argument parser: it starts, loads configuration,
        # and exits non-zero when required variables are absent. Reaching the
        # configuration parser proves the binary and its native dependencies load.
        http_probe = run(
            ["docker", "run", "--rm", "--entrypoint", HTTP_ENTRYPOINT, self.image],
            check=False,
            timeout=180,
        )
        if "config" not in (http_probe.stdout + http_probe.stderr):
            raise HarnessError(
                f"image {self.image!r} does not run {HTTP_ENTRYPOINT}; expected it to reach the "
                f"configuration parser (stdout/stderr tail: {(http_probe.stdout + http_probe.stderr)[-500:]})"
            )

    # ── disposable stack ───────────────────────────────────────────────
    def _write_protected(self, name: str, content: str) -> Path:
        path = self.work / name
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as handle:
            handle.write(content)
        return path

    def _generate_secrets(self) -> dict[str, str]:
        # Disposable, per-run values. Nothing here is committed or logged.
        return {
            "db_user": f"harness_{self.suffix}",
            "db_password": secrets.token_urlsafe(32),
            "pepper": secrets.token_urlsafe(48),
            "session_key": secrets.token_hex(32),
            "csrf_key": secrets.token_hex(32),
        }

    def start(self) -> None:
        self.work.chmod(0o700)
        secrets_map = self._generate_secrets()

        control_url = f"ws://{self.db_name}:8000/rpc"
        server_env = {
            "MEMORY_MCP_HTTP_BIND": "0.0.0.0:8080",
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL": PUBLIC_BASE_URL,
            "ALLOWED_HOSTS": f"localhost:{TLS_PORT}",
            "ALLOWED_ORIGINS": PUBLIC_BASE_URL,
            "MEMORY_MCP_HTTP_REPLICA_ID": "harness-1",
            # `crates/memory-mcp/src/http/server.rs` wraps the entire serve loop in
            # `tokio::time::timeout(shutdown_grace, serving)`, so the default 30s
            # grace acts as a total server lifetime and the process exits ~30s
            # after start (observed without any signal). A long grace keeps the
            # acceptance run alive; this is a harness workaround for that bug, not
            # a production recommendation.
            "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS": "3600",
            "MEMORY_MCP_API_KEY_PEPPER": secrets_map["pepper"],
            "MEMORY_MCP_HTTP_SESSION_KEY": secrets_map["session_key"],
            "MEMORY_MCP_HTTP_CSRF_KEY": secrets_map["csrf_key"],
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE": "true",
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI": "true",
            "MEMORY_MCP_HTTP_AUTH_METHODS": "local",
            "MEMORY_MCP_HTTP_SIGNUP_MODE": "invite_only",
            "MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION": "1",
            "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES": "1073741824",
            "MEMORY_MCP_HTTP_MAX_EPISODE_COUNT": "100000",
            "MEMORY_MCP_HTTP_INGEST_PER_MINUTE": "60",
            "MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS": "32",
            "MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS": "5",
            "MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY": "4",
            "MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY": "2",
            "SURREALDB_CONTROL_URL": control_url,
            "SURREALDB_CONTROL_USERNAME": secrets_map["db_user"],
            "SURREALDB_CONTROL_PASSWORD": secrets_map["db_password"],
            "SURREALDB_CONTROL_NAMESPACE": "control",
            "SURREALDB_CONTROL_DB": "registry",
            "SURREALDB_TENANT_URL": control_url,
            "SURREALDB_TENANT_USERNAME": secrets_map["db_user"],
            "SURREALDB_TENANT_PASSWORD": secrets_map["db_password"],
            "SURREALDB_TENANT_NAMESPACE": "tenant",
            "SURREALDB_TENANT_DB": "memory",
            "RUST_LOG": "info",
            "NER_EXTRACTOR": "anno",
            "EMBEDDINGS_ENABLED": "false",
        }
        self.server_env = server_env
        env_file = self._write_protected(
            "server.env", "".join(f"{k}={v}\n" for k, v in server_env.items())
        )

        print(f"harness: disposable stack on network {self.network}")
        run(["docker", "network", "create", self.network], timeout=60)

        # Disposable database. The official image defaults to nonroot but writes
        # a RocksDB directory under /data, so it runs as root here exactly as the
        # Compose service does.
        run(
            [
                "docker", "run", "-d", "--name", self.db_name,
                "--network", self.network,
                "--user", "0:0",
                SURREALDB_IMAGE,
                "start", "--user", secrets_map["db_user"], "--pass", secrets_map["db_password"],
                "rocksdb:/data/memory.db",
            ],
            timeout=300,
        )
        self.containers.append(self.db_name)
        self._wait_db()

        self._start_oidc_stub()

        # HTTP server: the image entrypoint, unchanged.
        run(
            [
                "docker", "run", "-d", "--name", self.http_name,
                "--network", self.network,
                "--env-file", str(env_file),
                self.image,
            ],
            timeout=180,
        )
        self.containers.append(self.http_name)

        tls = self._build_tls_material()
        self._start_tls_proxy(tls)
        self._wait_https(tls["ca_cert"])

        self.env_file = env_file
        self.ca_cert = tls["ca_cert"]
        self.ready = True

    def _wait_db(self) -> None:
        deadline = time.time() + 90
        while time.time() < deadline:
            probe = run(
                ["docker", "exec", self.db_name, "/surreal", "isready",
                 "--endpoint", "http://127.0.0.1:8000"],
                check=False,
                timeout=15,
            )
            if probe.returncode == 0:
                return
            time.sleep(2)
        raise HarnessError("disposable SurrealDB did not become ready")

    def _build_tls_material(self) -> dict[str, Path]:
        tls_dir = self.work / "tls"
        tls_dir.mkdir(mode=0o700)
        ca_key = tls_dir / "ca.key"
        ca_cert = tls_dir / "ca.crt"
        server_key = tls_dir / "server.key"
        server_csr = tls_dir / "server.csr"
        server_cert = tls_dir / "server.crt"
        ext = tls_dir / "ext.cnf"
        ext.write_text(
            "basicConstraints=critical,CA:FALSE\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\n"
            "extendedKeyUsage=serverAuth\n"
            "subjectAltName=DNS:localhost,IP:127.0.0.1\n"
        )

        run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-keyout", str(ca_key), "-out", str(ca_cert), "-days", "2",
             "-subj", "/CN=memory-mcp-local-admin-harness-ca",
             "-addext", "basicConstraints=critical,CA:TRUE",
             "-addext", "keyUsage=critical,keyCertSign,cRLSign"], timeout=120)
        run(["openssl", "req", "-newkey", "rsa:2048", "-nodes",
             "-keyout", str(server_key), "-out", str(server_csr),
             "-subj", "/CN=localhost"], timeout=120)
        run(["openssl", "x509", "-req", "-in", str(server_csr),
             "-CA", str(ca_cert), "-CAkey", str(ca_key), "-CAcreateserial",
             "-out", str(server_cert), "-days", "2", "-extfile", str(ext)], timeout=120)
        for path in (ca_key, server_key):
            path.chmod(0o600)
        return {"ca_cert": ca_cert, "server_cert": server_cert, "server_key": server_key}

    def _start_tls_proxy(self, tls: dict[str, Path]) -> None:
        caddyfile = self.work / "Caddyfile"
        caddyfile.write_text(
            "{\n\tauto_https off\n}\n"
            f"{PUBLIC_BASE_URL} {{\n"
            "\ttls /certs/server.crt /certs/server.key\n"
            f"\treverse_proxy {self.http_name}:8080\n"
            "}\n"
        )
        run(
            [
                "docker", "run", "-d", "--name", self.tls_name,
                "--network", self.network,
                "-p", f"127.0.0.1:{TLS_PORT}:{TLS_PORT}",
                "-v", f"{tls['server_cert'].parent}:/certs:ro",
                "-v", f"{caddyfile}:/etc/caddy/Caddyfile:ro",
                CADDY_IMAGE,
            ],
            timeout=300,
        )
        self.containers.append(self.tls_name)

    def _https_get(self, path: str, ca_cert: Path, timeout: int = 5):
        ctx = ssl.create_default_context(cafile=str(ca_cert))
        request = urllib.request.Request(f"{PUBLIC_BASE_URL}{path}")
        return urllib.request.urlopen(request, timeout=timeout, context=ctx)

    def _wait_https(self, ca_cert: Path) -> None:
        deadline = time.time() + 150
        last = ""
        while time.time() < deadline:
            try:
                self._https_get("/health/ready", ca_cert)
                return
            except Exception as error:  # noqa: BLE001 - report the last error
                last = str(error)
                time.sleep(2)
        logs = run(["docker", "logs", "--tail", "30", self.http_name], check=False).stdout
        raise HarnessError(
            "the HTTPS endpoint did not become ready; the image must be built with "
            "control-plane-ui and a real bundle for local browser auth.\n"
            f"last error: {last}\nhttp server logs (tail):\n{logs}"
        )

    # ── stub identity provider ─────────────────────────────────────────
    def oidc_issuer(self) -> str:
        """The issuer of the stub provider, as the server must configure it.

        The container's network name, not the loopback port the harness probes:
        the running server resolves it inside the deployment network, and the
        discovery document echoes this URL, which the client checks.
        """
        return f"http://{self.provider_name}"

    def _start_oidc_stub(self) -> None:
        """Serve `/.well-known/openid-configuration` for the scenarios that need it.

        `OidcClient::new` runs discovery against the configured issuer at startup
        and a failure there is fatal (`crates/memory-mcp/src/http.rs`), so an
        `oidc`-enabled deployment cannot boot without a reachable issuer even for
        a run that never completes a login. The document is served by the Caddy
        image the harness already requires; the issuer is the container's own
        network name, so no published port is needed for the server to reach it.
        """
        issuer = self.oidc_issuer()
        document = {
            "issuer": issuer,
            "authorization_endpoint": f"{issuer}/authorize",
            "token_endpoint": f"{issuer}/token",
            "jwks_uri": f"{issuer}/jwks",
            "id_token_signing_alg_values_supported": ["RS256"],
        }
        root = self.work / "oidc" / ".well-known"
        root.mkdir(parents=True)
        (root / "openid-configuration").write_text(json.dumps(document) + "\n")
        run(
            [
                "docker", "run", "-d", "--name", self.provider_name,
                "--network", self.network,
                "-p", f"127.0.0.1:{OIDC_PORT}:80",
                "-v", f"{self.work / 'oidc'}:/srv:ro",
                CADDY_IMAGE,
                "caddy", "file-server", "--root", "/srv", "--listen", ":80",
            ],
            timeout=300,
        )
        self.containers.append(self.provider_name)
        self._wait_oidc_stub()

    def _wait_oidc_stub(self) -> None:
        url = f"http://127.0.0.1:{OIDC_PORT}/.well-known/openid-configuration"
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                with urllib.request.urlopen(url, timeout=5) as response:
                    if response.status == 200:
                        return
            except Exception:  # noqa: BLE001 - the retry loop owns the diagnosis
                time.sleep(2)
        raise HarnessError("the stub identity provider did not serve its discovery document")

    # ── execution ──────────────────────────────────────────────────────
    def _cli(self, args: list[str], *, timeout: int = 120, env: dict[str, str] | None = None):
        """Run the CLI from the image. Secret stdout is returned, never printed.

        `env` entries are passed as `-e` overrides, which Docker applies after the
        `--env-file` snapshot of the running deployment. That is how a scenario
        presents the *target* configuration to a command without restarting the
        server first, which is the order ADR-0057 documents.
        """
        overrides = [
            item
            for key, value in (env or {}).items()
            for item in ("-e", f"{key}={value}")
        ]
        return run(
            [
                "docker", "run", "--rm", "--network", self.network,
                "--env-file", str(self.env_file),
                *overrides,
                "--entrypoint", CLI_IN_IMAGE, self.image,
                *args,
            ],
            timeout=timeout,
            check=False,
        )

    # ── harness-owned scenarios ───────────────────────────────────────
    def _https_request(self, path: str) -> tuple[int, str]:
        """One HTTPS request against the deployment boundary.

        Redirects are not followed: a disabled method answers `404` while an
        enabled one may answer `3xx`, and a followed redirect would turn the
        second case into a network error against an unreachable provider.
        """
        context = ssl.create_default_context(cafile=str(self.ca_cert))
        connection = http.client.HTTPSConnection(
            "localhost", TLS_PORT, context=context, timeout=15
        )
        try:
            connection.request("GET", path)
            response = connection.getresponse()
            return response.status, response.read().decode("utf-8", "replace")
        finally:
            connection.close()

    def _expect(self, condition: bool, message: str) -> None:
        if not condition:
            raise HarnessError(message)

    def _restart_server(self, overrides: dict[str, str]) -> None:
        """Replace the HTTP container, keeping the registry and the network.

        The base environment is the one the deployment started with, so a
        scenario can change exactly the variables it is about. The container keeps
        its name, which is what the TLS proxy resolves.
        """
        merged = dict(self.server_env)
        merged.update(overrides)
        env_file = self._write_protected(
            "server-restart.env", "".join(f"{k}={v}\n" for k, v in merged.items())
        )
        run(["docker", "rm", "-f", self.http_name], check=False, timeout=120)
        run(
            [
                "docker", "run", "-d", "--name", self.http_name,
                "--network", self.network,
                "--env-file", str(env_file),
                self.image,
            ],
            timeout=180,
        )
        self._wait_https(self.ca_cert)

    def _provider_overlay(self, operator_subject: str = "ab12") -> dict[str, str]:
        """The material the `oidc` method needs, pointing at the stub provider.

        Present whenever `oidc` is in the set, including beside `local`: the three
        keys are derived from the session key *only* while `local` is the whole
        set, so enabling a second method is what makes them real key material.
        """
        issuer = self.oidc_issuer()
        return {
            "MEMORY_MCP_HTTP_SIGNUP_MODE": "invite_only",
            "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES": f"{issuer}|{operator_subject}",
            "MEMORY_MCP_HTTP_OIDC_ISSUER": issuer,
            "MEMORY_MCP_HTTP_OIDC_CLIENT_ID": "harness-client",
            "MEMORY_MCP_HTTP_OIDC_AUDIENCE": PUBLIC_BASE_URL,
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI": f"{PUBLIC_BASE_URL}/auth/oidc/callback",
            "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY": secrets.token_hex(32),
            "MEMORY_MCP_HTTP_OIDC_STATE_KEY": secrets.token_hex(32),
            "MEMORY_MCP_HTTP_OIDC_NONCE_KEY": secrets.token_hex(32),
        }

    def scenario_removal(self) -> None:
        """Reach "SSO only" end to end: add the provider, then drop the door.

        The removal is a CLI operation by design, so this scenario runs the shipped
        binary against the deployment's own durable registry and restarts the
        server around it. It walks the whole ADR-0057 story in order, because the
        store only narrows a set that has something to narrow to: `local` alone
        cannot be removed, so the provider has to be added beside it first. That
        precondition is asserted here rather than assumed.

        The in-crate `tests/local_admin_cli.rs` covers the same sequence against a
        file-backed registry without a running deployment; this is the container
        and route-level counterpart.
        """
        operator = self._provider_overlay()["MEMORY_MCP_HTTP_OPERATOR_IDENTITIES"]
        target = {"MEMORY_MCP_HTTP_AUTH_METHODS": "oidc"}
        removal = ["admin", "auth-methods", "remove", "--method", "local"]
        authorised = {**target, "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES": operator}

        def policy() -> str:
            status, body = self._https_request("/api/v1/auth/config")
            self._expect(status == 200, f"/api/v1/auth/config returned {status}")
            return body

        def expect_mounted(paths, mounted: bool) -> None:
            for path in paths:
                status, _ = self._https_request(path)
                self._expect(
                    (status != 404) == mounted,
                    f"{path} must be {'mounted' if mounted else 'absent'}, got {status}",
                )

        local_routes = ("/api/v1/auth/local/csrf", "/api/v1/admin/session")
        provider_routes = ("/auth/oidc/authorize", "/api/v1/account")

        # 1. The deployment starts on the local door alone.
        body = policy()
        self._expect('"local"' in body, f"the disclosure must name local: {body}")
        self._expect('"oidc"' not in body, f"oidc is not enabled in this run: {body}")
        expect_mounted(local_routes, mounted=True)
        expect_mounted(provider_routes, mounted=False)

        # 2. Guard one: the configuration still enables the method, and startup
        # reconciliation is additive, so removing it now would be undone.
        refused = self._cli(
            removal,
            env={
                "MEMORY_MCP_HTTP_AUTH_METHODS": "local",
                "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES": operator,
            },
        )
        self._expect(
            refused.returncode != 0, "a method the configuration still enables must be refused"
        )
        self._expect(
            "still enabled" in refused.stderr,
            f"the refusal must say the method is still enabled: {refused.stderr[-500:]}",
        )

        # 3. Guard two: the last-administrator rule. Without an operator identity, a
        # local-free deployment would have nobody able to administer it.
        refused = self._cli(removal, env=target)
        self._expect(refused.returncode != 0, "removing local without an operator must be refused")
        self._expect(
            "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES" in refused.stderr,
            f"the refusal must name the variable that fixes it: {refused.stderr[-500:]}",
        )

        # 4. The store's own rule, which the two guards cannot express: the policy
        # may not be narrowed to nothing. This is the precondition of the whole
        # flow, and it is why the provider is added before the door is removed.
        premature = self._cli(removal, env=authorised)
        self._expect(
            premature.returncode != 0,
            "the last remaining method must not be removable",
        )
        self._expect(
            "would leave no browser authentication method" in premature.stderr,
            f"the store must refuse to empty the policy: {premature.stderr[-500:]}",
        )

        # 5. Add the provider beside the door: one environment change, one restart,
        # and the local method keeps working through it.
        self._restart_server(
            {
                "MEMORY_MCP_HTTP_AUTH_METHODS": "local,oidc",
                **self._provider_overlay(),
            }
        )
        body = policy()
        self._expect('"local"' in body and '"oidc"' in body, f"both methods now: {body}")
        expect_mounted(local_routes, mounted=True)
        expect_mounted(provider_routes, mounted=True)
        print("harness: the local door and the provider are served side by side")

        # 6. The removal itself.
        removed = self._cli(removal, env=authorised)
        self._expect(
            removed.returncode == 0,
            f"removing local beside an enabled provider must succeed: {removed.stderr[-500:]}",
        )
        try:
            report = json.loads(removed.stdout)
        except json.JSONDecodeError as error:
            raise HarnessError(f"the removal did not return JSON: {error}") from error
        self._expect(report.get("removed_method") == "local", f"unexpected report: {report}")
        self._expect(report.get("enabled_methods") == "oidc", f"unexpected report: {report}")
        self._expect(
            str(report.get("epoch", "")).isdigit(),
            f"the removal must report the advanced epoch: {report}",
        )
        guidance = str(report.get("guidance", ""))
        self._expect(
            "MEMORY_MCP_HTTP_AUTH_METHODS=oidc" in guidance,
            f"the command must print the configuration to deploy: {guidance}",
        )
        print(f"harness: the removal advanced the policy epoch to {report['epoch']}")

        # Its success is also the evidence that none of the four refusals above
        # wrote anything: the store refuses a second removal by name.
        again = self._cli(removal, env=authorised)
        self._expect(again.returncode != 0, "a method cannot be removed twice")
        self._expect(
            "not enabled by the durable policy" in again.stderr,
            f"the store must report the absent method: {again.stderr[-500:]}",
        )

        # 7. The durable change decides what the deployment serves: restart with the
        # narrowed set and the surfaces have swapped.
        self._restart_server({**target, **self._provider_overlay()})
        body = policy()
        self._expect('"oidc"' in body, f"the disclosure must name oidc: {body}")
        self._expect('"local"' not in body, f"the local method is gone: {body}")
        expect_mounted(local_routes, mounted=False)
        expect_mounted(provider_routes, mounted=True)
        print("harness: the restarted deployment serves oidc only")

    def verify_cli(self) -> None:
        print("harness: invoking the source-built CLI in the image (output captured in memory)")
        probe = self._cli(["admin", "create", "--username", f"harness.probe.{self.suffix}"])
        if probe.returncode != 0:
            raise HarnessError(
                f"the CLI `admin create` failed; the image must ship memory_mcp built with "
                f"streamable-http (stderr: {probe.stderr[-500:]})"
            )
        try:
            issued = json.loads(probe.stdout)
        except json.JSONDecodeError as error:
            raise HarnessError(f"CLI did not return JSON: {error}") from error
        if not isinstance(issued.get("code"), str) or not issued["code"]:
            raise HarnessError("CLI admin create did not return a one-time code")
        # The code is deliberately not printed and not written anywhere.
        print("harness: CLI issued a one-time code (redacted)")

    def write_fixture(self) -> Path:
        fixture = {
            "base_url": PUBLIC_BASE_URL,
            "tls": {"mode": "ca_pem", "ca_pem_path": str(self.ca_cert)},
            "cli": {
                "argv_prefix": [
                    "docker", "run", "--rm",
                    "--network", self.network,
                    "--env-file", str(self.env_file),
                    "--entrypoint", CLI_IN_IMAGE,
                    self.image,
                ],
                "timeout_seconds": 120,
            },
            "admin": {"username_prefix": f"browser.{self.suffix}."},
            "server": {"container": self.http_name},
        }
        path = self._write_protected("browser-fixture.json", json.dumps(fixture, indent=2) + "\n")
        return path

    def run_scenarios(self) -> None:
        fixture = self.write_fixture()
        env = dict(os.environ, LOCAL_ADMIN_BROWSER_FIXTURE=str(fixture))
        for scenario in self.scenarios:
            if scenario in HARNESS_SCENARIOS:
                print(f"harness: scenario {scenario}")
                getattr(self, f"scenario_{scenario}")()
                continue
            print(f"harness: browser scenario {scenario}")
            result = run(
                ["node", str(BROWSER_RUNNER), "--base-url", PUBLIC_BASE_URL, "--scenario", scenario],
                check=False,
                timeout=600,
                env=env,
            )
            if result.stdout:
                print(result.stdout.rstrip())
            if result.returncode != 0:
                if result.stderr:
                    print(result.stderr.rstrip(), file=sys.stderr)
                raise HarnessError(
                    f"browser scenario {scenario!r} failed (exit {result.returncode}); "
                    "the runner must be installed and the image must bundle the local UI"
                )
        print(f"harness: all scenarios passed: {', '.join(self.scenarios)}")

    # ── teardown ───────────────────────────────────────────────────────
    def cleanup(self) -> None:
        if self.keep:
            print(f"harness: --keep set; leaving containers, network {self.network} and {self.work}")
            return
        for name in reversed(self.containers):
            run(["docker", "rm", "-f", name], check=False, timeout=120)
        run(["docker", "network", "rm", self.network], check=False, timeout=60)
        shutil.rmtree(self.work, ignore_errors=True)


def parse_scenarios(value: str) -> list[str]:
    if value == "all":
        return list(ALL_SCENARIOS)
    selected = [part.strip() for part in value.split(",") if part.strip()]
    unknown = [part for part in selected if part not in ALL_SCENARIOS]
    if not selected or unknown:
        raise argparse.ArgumentTypeError(
            f"--scenario must be 'all' or a comma-separated subset of {', '.join(ALL_SCENARIOS)}"
        )
    return selected


def main() -> int:
    parser = argparse.ArgumentParser(description="Local-admin image acceptance harness")
    parser.add_argument("--image", required=True, help="image to test (built from this repo)")
    parser.add_argument("--scenario", default="all", type=parse_scenarios)
    parser.add_argument("--keep", action="store_true", help="keep the disposable stack for debugging")
    args = parser.parse_args()

    harness = Harness(args.image, args.scenario, args.keep)
    exit_code = 0
    try:
        harness.check_prerequisites()
        harness.start()
        harness.verify_cli()
        harness.run_scenarios()
    except HarnessError as error:
        print(f"local_admin_image: FAILED: {error}", file=sys.stderr)
        exit_code = 1
    except KeyboardInterrupt:
        print("local_admin_image: interrupted", file=sys.stderr)
        exit_code = 130
    finally:
        harness.cleanup()
    if exit_code == 0:
        print("local_admin_image: OK")
    return exit_code


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main())
