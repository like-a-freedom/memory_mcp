#!/usr/bin/env node
/**
 * Local-admin browser acceptance runner.
 *
 *   node scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario auth
 *
 * Scenarios:
 *   auth       — CLI-issued activation code, pre-auth CSRF, activation, login,
 *                session, logout, and post-logout rejection.
 *   clients    — login, create client, issue/revoke a key, suspend/resume,
 *                stale-CAS rejection, and "no secret after refresh".
 *   regression — negative transport checks: missing CSRF, wrong Origin, OIDC
 *                route 404, operator route 404, unmatched API 404.
 *   ui         — loads the packaged Dioxus/WASM bundle in a real page and
 *                asserts it boots under the shipped Content-Security-Policy
 *                (index.html bytes alone are not evidence that WebAssembly
 *                executed).
 *
 * All runs require a protected fixture file:
 *   LOCAL_ADMIN_BROWSER_FIXTURE=/path/to/fixture.json
 *
 * The fixture identifies the disposable CLI/container and the TLS trust for the
 * endpoint. It never carries a production password; the runner generates its
 * own disposable admin password in memory and obtains fresh activation codes by
 * invoking the CLI described by the fixture.
 *
 * Dependency gate: this runner uses the pinned `playwright` version declared by
 * scripts/ci/package.json + scripts/ci/package-lock.json. If it is not
 * installed the runner exits non-zero with the exact install command. It never
 * skips and never exits 0 without running the requested scenario.
 */

import { parseArgs } from 'node:util';
import { randomBytes, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const SCENARIOS = ['auth', 'clients', 'regression', 'ui'];

function fail(message) {
  console.error(`local_admin_browser: ERROR: ${message}`);
  process.exit(1);
}

const { values } = parseArgs({
  options: {
    'base-url': { type: 'string', default: 'https://localhost:8443' },
    scenario: { type: 'string', default: 'auth' },
  },
  strict: false,
});

const BASE_URL = String(values['base-url'] ?? 'https://localhost:8443').replace(/\/$/, '');
const SCENARIO = String(values.scenario ?? 'auth');

if (!SCENARIOS.includes(SCENARIO)) {
  fail(`unknown scenario '${SCENARIO}'; expected one of ${SCENARIOS.join(', ')}`);
}
if (!BASE_URL.startsWith('https://')) {
  fail(`--base-url must be an https:// URL (got ${BASE_URL}); secure __Host- cookies require TLS`);
}

// ── Dependency gate ────────────────────────────────────────────────────────
// Resolve the pinned package relative to this script so the runner works from
// any working directory. A missing dependency is a hard failure, never a skip.
let chromium;
try {
  ({ chromium } = await import('playwright'));
} catch (error) {
  const manifest = resolve(HERE, 'package.json');
  fail(
    'prerequisites not installed: the pinned browser runner package is missing.\n' +
      `  run: (cd ${HERE} && npm ci && npx playwright install --with-deps chromium)\n` +
      `  manifest: ${existsSync(manifest) ? manifest : '(missing scripts/ci/package.json)'}\n` +
      `  underlying error: ${error.message}`,
  );
}

// ── Fixture gate ───────────────────────────────────────────────────────────
const fixturePath = process.env.LOCAL_ADMIN_BROWSER_FIXTURE;
if (!fixturePath) {
  fail(
    'LOCAL_ADMIN_BROWSER_FIXTURE is not set.\n' +
      '  The image harness supplies it; a bare URL is not sufficient because the runner must\n' +
      '  invoke a disposable CLI for fresh codes and know the TLS trust for the endpoint.',
  );
}
if (!existsSync(fixturePath)) {
  fail(`LOCAL_ADMIN_BROWSER_FIXTURE points at a missing file: ${fixturePath}`);
}

let fixture;
try {
  fixture = JSON.parse(readFileSync(fixturePath, 'utf8'));
} catch (error) {
  fail(`cannot parse fixture ${fixturePath}: ${error.message}`);
}

if (!fixture.cli || !Array.isArray(fixture.cli.argv_prefix) || fixture.cli.argv_prefix.length === 0) {
  fail('fixture.cli.argv_prefix must be a non-empty array describing the disposable CLI invocation');
}
if (!fixture.admin || typeof fixture.admin.username_prefix !== 'string') {
  fail('fixture.admin.username_prefix must be a string');
}
if (fixture.base_url && String(fixture.base_url).replace(/\/$/, '') !== BASE_URL) {
  fail(`fixture.base_url (${fixture.base_url}) does not match --base-url (${BASE_URL})`);
}
const tlsMode = fixture.tls?.mode ?? 'insecure';
if (tlsMode === 'ca_pem' && !existsSync(fixture.tls.ca_pem_path ?? '')) {
  fail('fixture.tls.mode is ca_pem but tls.ca_pem_path does not exist');
}

// ── Redaction helpers ──────────────────────────────────────────────────────
// Secrets live only in memory. Every credential this runner mints is
// registered here, and any occurrence of one is replaced before output. Only
// registered values and credential-*shaped* strings are redacted, so a
// diagnostic that merely happens to be long still reaches the operator.
const SECRET_KEYS = /(secret|password|code|token|api[-_]?key|csrf)/i;
const SECRET_VALUES = new Set();

/** Register a minted credential whose literal must never reach output. */
function registerSecret(value) {
  if (typeof value === 'string' && value.length > 0) SECRET_VALUES.add(value);
  return value;
}

/** Long unbroken hex/base64 runs are credential-shaped. */
const SECRET_SHAPED = /^[A-Za-z0-9+/_=-]{24,}$/;

function redactString(value) {
  let out = value;
  for (const secret of SECRET_VALUES) {
    if (out.includes(secret)) out = out.split(secret).join('[redacted]');
  }
  if (SECRET_SHAPED.test(out)) return '[redacted]';
  return out.length > 400 ? `${out.slice(0, 400)}…` : out;
}

function redact(value) {
  if (value === null || value === undefined) return value;
  if (typeof value === 'string') return redactString(value);
  if (Array.isArray(value)) return value.map(redact);
  if (typeof value === 'object') {
    const out = {};
    for (const [key, entry] of Object.entries(value)) {
      out[key] = SECRET_KEYS.test(key) ? '[redacted]' : redact(entry);
    }
    return out;
  }
  return value;
}

let checks = 0;
function check(label, condition, detail) {
  checks += 1;
  if (condition) {
    console.log(`  ok   ${label}`);
    return;
  }
  console.error(`  FAIL ${label}${detail === undefined ? '' : ` :: ${JSON.stringify(redact(detail))}`}`);
  throw new Error(`assertion failed: ${label}`);
}

// ── Disposable CLI access ──────────────────────────────────────────────────
function cliJson(args) {
  const timeout = (fixture.cli.timeout_seconds ?? 90) * 1000;
  let stdout;
  try {
    stdout = execFileSync(fixture.cli.argv_prefix[0], [...fixture.cli.argv_prefix.slice(1), ...args], {
      encoding: 'utf8',
      timeout,
      // Secret stdout stays in this variable; it is never written to a file or logged.
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) {
    // The CLI prints the one-time code on stdout; do not echo either stream.
    fail(`CLI invocation failed (${args[0]} ${args[1] ?? ''}); stderr suppressed to avoid leaking codes`);
  }
  try {
    return JSON.parse(stdout);
  } catch {
    fail('CLI did not return JSON; output suppressed to avoid leaking a one-time code');
  }
}

function freshCode() {
  const username = `${fixture.admin.username_prefix}${randomBytes(3).toString('hex')}`;
  const issued = cliJson(['admin', 'create', '--username', username]);
  if (!issued || typeof issued.code !== 'string' || issued.code.length === 0) {
    fail('CLI create did not return a code');
  }
  return { username, code: registerSecret(issued.code) };
}

function disposablePassword() {
  // Never logged, never persisted; discarded when this process exits.
  return registerSecret(`disp-${randomBytes(24).toString('hex')}`);
}

async function activateAndLogin(context, username, code, password) {
  const csrfResponse = await context.request.get(`${BASE_URL}/api/v1/auth/local/csrf`);
  check('pre-auth csrf returns 200', csrfResponse.status() === 200, csrfResponse.status());
  const { csrf_token: csrfToken } = await csrfResponse.json();
  registerSecret(csrfToken);
  check('pre-auth csrf token non-empty', typeof csrfToken === 'string' && csrfToken.length > 0);

  const activate = await context.request.post(`${BASE_URL}/api/v1/auth/local/activate`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
    data: { code, password },
  });
  check('activation returns 204', activate.status() === 204, activate.status());

  // A fresh pre-auth cookie is required for the login POST.
  const csrf2Response = await context.request.get(`${BASE_URL}/api/v1/auth/local/csrf`);
  const { csrf_token: csrf2 } = await csrf2Response.json();
  registerSecret(csrf2);
  const login = await context.request.post(`${BASE_URL}/api/v1/auth/local/login`, {
    headers: { 'X-CSRF-Token': csrf2, Origin: BASE_URL },
    data: { username, password },
  });
  check('login returns 204', login.status() === 204, login.status());

  const session = await context.request.get(`${BASE_URL}/api/v1/admin/session`);
  check('session returns 200', session.status() === 200, session.status());
  const sessionBody = await session.json();
  registerSecret(sessionBody.csrf_token);
  check('session exposes a csrf token', typeof sessionBody.csrf_token === 'string');
  return { csrfToken: sessionBody.csrf_token, adminId: sessionBody.admin_id };
}

// ── Scenarios ──────────────────────────────────────────────────────────────
async function scenarioAuth(context) {
  const config = await context.request.get(`${BASE_URL}/api/v1/auth/config`);
  const body = await config.json();
  check('auth config reports local mode', body.mode === 'local', body);

  const { username, code } = freshCode();
  const password = disposablePassword();
  const { csrfToken } = await activateAndLogin(context, username, code, password);

  const logout = await context.request.post(`${BASE_URL}/api/v1/admin/logout`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
  });
  check('logout returns 204', logout.status() === 204, logout.status());

  const after = await context.request.get(`${BASE_URL}/api/v1/admin/session`);
  check('session is rejected after logout', after.status() === 401, after.status());
}

async function scenarioClients(context) {
  const { username, code } = freshCode();
  const password = disposablePassword();
  const { csrfToken } = await activateAndLogin(context, username, code, password);

  const idempotency = randomUUID();
  const created = await context.request.post(`${BASE_URL}/api/v1/admin/clients`, {
    headers: {
      'X-CSRF-Token': csrfToken,
      Origin: BASE_URL,
      'Idempotency-Key': idempotency,
    },
    data: { display_name: `browser${randomBytes(3).toString('hex')}` },
  });
  check('client create returns 202', created.status() === 202, created.status());
  const location = created.headers()['location'];
  check('client create returns a Location header', typeof location === 'string' && location.length > 0);
  const clientId = location.split('/').filter(Boolean).pop();

  // Provisioning is asynchronous: poll the visible nonterminal state until the
  // tenant is ready (or a terminal failure is reported). The poll's status is
  // reported once, after the loop, so the check count does not depend on how
  // many polls the deployment happened to need.
  let clientView = null;
  let finalReadStatus = 0;
  const deadline = Date.now() + 45_000;
  while (Date.now() < deadline) {
    const read = await context.request.get(`${BASE_URL}${location}`);
    finalReadStatus = read.status();
    if (finalReadStatus !== 200) break;
    clientView = await read.json();
    if (clientView.tenant_status === 'ready') break;
    if (clientView.tenant_status === 'failed') {
      fail(`client provisioning failed: ${clientView.provisioning_reason ?? 'no reason'}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  check('client read returns 200', finalReadStatus === 200, finalReadStatus);
  check('client reaches ready', clientView?.tenant_status === 'ready', clientView?.tenant_status);

  // Issue a key. The one-time secret exists only in this variable
  // (`revealedSecret`) and is never printed and never screenshotted.
  const issued = await context.request.post(`${BASE_URL}/api/v1/admin/clients/${clientId}/keys`, {
    headers: {
      'X-CSRF-Token': csrfToken,
      Origin: BASE_URL,
      'Idempotency-Key': randomUUID(),
    },
    data: { name: 'browser-key', expiry: { kind: 'never' } },
  });
  check('key issue returns 201', issued.status() === 201, issued.status());
  const issuedBody = await issued.json();
  const issuedKeyId = issuedBody.id;
  const revealedSecret = registerSecret(issuedBody.secret);
  check('key issue returns a one-time secret', typeof revealedSecret === 'string' && revealedSecret.length > 0);
  check('key issue returns a key id', typeof issuedKeyId === 'string' && issuedKeyId.length > 0);

  const listed = await context.request.get(`${BASE_URL}/api/v1/admin/clients/${clientId}/keys`);
  check('key list returns 200', listed.status() === 200, listed.status());
  const listedBody = await listed.json();
  const listedItems = Array.isArray(listedBody) ? listedBody : listedBody.items ?? [];
  check('key list includes the issued key', listedItems.some((item) => item.id === issuedKeyId));
  check(
    'key list never replays the secret',
    !JSON.stringify(listedItems).includes(revealedSecret),
  );

  const revoked = await context.request.delete(
    `${BASE_URL}/api/v1/admin/clients/${clientId}/keys/${issuedKeyId}`,
    { headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL } },
  );
  check('key revoke returns 204', revoked.status() === 204, revoked.status());

  // Suspend/resume is version-checked, with one documented exception:
  // repeating a request that is already at the desired state is a coherent
  // no-op (204, no writes) even when the version in the body is stale.
  const current = await context.request.get(`${BASE_URL}${location}`);
  const version = (await current.json()).version;

  // While the client is still in the suspend source state, a stale version
  // must be refused rather than silently applied.
  const staleBefore = await context.request.post(
    `${BASE_URL}/api/v1/admin/clients/${clientId}/suspend`,
    {
      headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
      data: { expected_version: Math.max(version - 1, 0) },
    },
  );
  check('stale suspend is refused with 409', staleBefore.status() === 409, staleBefore.status());

  const suspend = await context.request.post(`${BASE_URL}/api/v1/admin/clients/${clientId}/suspend`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
    data: { expected_version: version },
  });
  check('suspend accepts the current version', suspend.status() === 204, suspend.status());

  const suspended = await context.request.get(`${BASE_URL}${location}`);
  const suspendedView = await suspended.json();
  check('suspend suspends the account', suspendedView.account_status === 'suspended', suspendedView.account_status);
  check('suspend suspends the tenant', suspendedView.tenant_status === 'suspended', suspendedView.tenant_status);
  check(
    'suspend advances the guard version',
    suspendedView.version === version + 1,
    suspendedView.version,
  );

  // The same request again is a coherent no-op: the stale version is expected
  // to be tolerated, and the guard version must not move.
  const repeated = await context.request.post(`${BASE_URL}/api/v1/admin/clients/${clientId}/suspend`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
    data: { expected_version: version },
  });
  check('repeating suspend is a coherent no-op', repeated.status() === 204, repeated.status());
  const afterRepeat = await context.request.get(`${BASE_URL}${location}`);
  check(
    'a coherent no-op does not advance the guard version',
    (await afterRepeat.json()).version === version + 1,
  );

  const resume = await context.request.post(`${BASE_URL}/api/v1/admin/clients/${clientId}/resume`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
    data: { expected_version: version + 1 },
  });
  check('resume accepts the refreshed version', resume.status() === 204, resume.status());
  const resumed = await context.request.get(`${BASE_URL}${location}`);
  const resumedView = await resumed.json();
  check('resume restores the account', resumedView.account_status === 'active', resumedView.account_status);
  check('resume restores the tenant', resumedView.tenant_status === 'ready', resumedView.tenant_status);
  check(
    'a suspended tenant is not reprovisioned while suspended',
    suspendedView.schema_version === resumedView.schema_version,
    `${suspendedView.schema_version} -> ${resumedView.schema_version}`,
  );

  // Back in the suspend source state, the pre-suspend version is stale again
  // and must be refused.
  const staleAfter = await context.request.post(`${BASE_URL}/api/v1/admin/clients/${clientId}/suspend`, {
    headers: { 'X-CSRF-Token': csrfToken, Origin: BASE_URL },
    data: { expected_version: version },
  });
  check('stale suspend after resume is refused with 409', staleAfter.status() === 409, staleAfter.status());
}

async function scenarioRegression(context) {
  const config = await context.request.get(`${BASE_URL}/api/v1/auth/config`);
  check('auth config is served', config.status() === 200, config.status());

  const noCsrf = await context.request.post(`${BASE_URL}/api/v1/admin/clients`, {
    headers: { Origin: BASE_URL, 'Idempotency-Key': randomUUID() },
    data: { display_name: 'regression' },
  });
  check('sessionless create without CSRF is 403/401', [401, 403].includes(noCsrf.status()), noCsrf.status());

  const wrongOrigin = await context.request.post(`${BASE_URL}/api/v1/auth/local/login`, {
    headers: { Origin: 'https://evil.example' },
    data: { username: 'nobody', password: 'irrelevant-but-long-enough' },
  });
  check('cross-origin mutation is refused with 403', wrongOrigin.status() === 403, wrongOrigin.status());

  // OIDC browser routes are not mounted in local mode. With the SPA enabled an
  // unknown *page* path is served by the client-side fallback (index.html,
  // 200 text/html); the security property is that no OIDC flow is reachable —
  // never an IdP redirect or an OIDC handler response.
  const oidc = await context.request.get(`${BASE_URL}/auth/oidc/authorize`);
  const oidcType = oidc.headers()['content-type'] ?? '';
  check(
    'OIDC authorize exposes no OIDC flow in local mode',
    oidc.status() === 404 || (oidc.status() === 200 && oidcType.includes('text/html')),
    `${oidc.status()} ${oidcType}`,
  );

  // Unmounted *API* routes must be a real 404, never a SPA 200.
  const operator = await context.request.get(`${BASE_URL}/api/v1/operator/recovery/status`);
  check('operator recovery is 404 in local mode', operator.status() === 404, operator.status());

  const unmatched = await context.request.get(`${BASE_URL}/api/v1/not-a-route`);
  check('unmatched API is 404 (not SPA 200)', unmatched.status() === 404, unmatched.status());
}

async function scenarioUi(context) {
  // A real page load: `context.request` never executes script, so only this
  // scenario can prove the WASM bundle runs under the shipped CSP.
  const page = await context.newPage();
  const consoleErrors = [];
  const pageErrors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => pageErrors.push(error.message));

  const response = await page.goto(`${BASE_URL}/admin/login`, { waitUntil: 'load' });
  check('ui document is served', response?.status() === 200, response?.status());
  const contentType = response?.headers()['content-type'] ?? '';
  check('ui document is html', contentType.includes('text/html'), contentType);

  // Assert the *served* policy, not just that the page happened to boot: a
  // future relaxation to blanket `unsafe-eval` must fail here.
  const csp = response?.headers()['content-security-policy'] ?? '';
  const scriptSrc =
    csp
      .split(';')
      .map((part) => part.trim())
      .find((part) => part.startsWith('script-src')) ?? '';
  check('csp allows wasm compilation', scriptSrc.includes("'wasm-unsafe-eval'"), scriptSrc || csp);
  check('csp refuses general eval', !/(^|\s)'unsafe-eval'/.test(csp), csp);
  check('csp refuses inline script', !/(^|\s)'unsafe-inline'/.test(csp), csp);

  // The Dioxus app is client-rendered, so the login form existing in the DOM
  // is the evidence that the JS and WASM bundle both executed. A CSP or MIME
  // refusal leaves the mount point empty.
  let booted = true;
  try {
    await page.waitForSelector('#admin-username', { state: 'visible', timeout: 30_000 });
  } catch {
    booted = false;
  }
  check(
    'wasm app boots under the shipped csp',
    booted,
    booted ? 'mounted' : `console=${JSON.stringify(consoleErrors.slice(0, 3))}`,
  );
  check('bundle produced no csp violations', consoleErrors.length === 0, consoleErrors.slice(0, 3));
  check('bundle raised no uncaught errors', pageErrors.length === 0, pageErrors.slice(0, 3));

  const heading = await page.textContent('h1').catch(() => null);
  check(
    'login page renders the administrator heading',
    (heading ?? '').includes('Administrator sign-in'),
    heading,
  );

  // The shipped pages must not pull third-party assets (no analytics/CDN).
  const external = await page.evaluate(() =>
    performance
      .getEntriesByType('resource')
      .map((entry) => entry.name)
      .filter((name) => /^https?:\/\//.test(name) && !name.startsWith(window.location.origin)),
  );
  check('no external assets are loaded', external.length === 0, external.slice(0, 3));

  await page.close();
}

// ── Driver ─────────────────────────────────────────────────────────────────
console.log(`local_admin_browser: base=${BASE_URL} scenario=${SCENARIO} tls=${tlsMode}`);

const browser = await chromium.launch({ headless: true });
let exitCode = 0;
try {
  const context = await browser.newContext({
    // The disposable endpoint uses a self-signed certificate. Chromium cannot
    // load an arbitrary CA without OS-level trust, so certificate errors are
    // ignored for the harness; the fixture still pins which endpoint/CA the
    // disposable stack uses. Nothing here is a production trust decision.
    ignoreHTTPSErrors: true,
  });
  context.setDefaultTimeout(20_000);
  try {
    if (SCENARIO === 'auth') await scenarioAuth(context);
    else if (SCENARIO === 'clients') await scenarioClients(context);
    else if (SCENARIO === 'ui') await scenarioUi(context);
    else await scenarioRegression(context);
    console.log(`local_admin_browser: ${checks} checks passed`);
  } finally {
    // Clean up sensitive browser state: no storage state, cookies, or traces
    // are persisted, and the context is closed whether or not a check failed.
    await context.close();
  }
} catch (error) {
  console.error(`local_admin_browser: FAILED: ${error.message}`);
  exitCode = 1;
} finally {
  await browser.close();
}

process.exit(exitCode);
