// Browser integration regressions for account flows and shared console CSS
// against the compiled Dioxus app and controlled APIs. No live credential is used.
//
// Build: dx bundle --platform web --package ui --locked --out-dir /tmp/memory-ui
// Run:   node crates/ui/tests/api_keys.browser.mjs /tmp/memory-ui/public [capture-dir]
// Requires Playwright and its Chromium. MEMORY_MCP_PLAYWRIGHT_MODULE can name
// an existing Playwright installation without changing Cargo dependencies.
// MEMORY_MCP_CHROMIUM_EXECUTABLE can select an installed Chrome instead.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve, extname, sep } from 'node:path';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.MEMORY_MCP_PLAYWRIGHT_MODULE || 'playwright');
assert(process.argv[2], 'Pass the dx bundle output directory');
const bundle = resolve(process.argv[2]);
const captures = process.argv[3] && resolve(process.argv[3]);
const selected = (name) => !process.env.MEMORY_MCP_BROWSER_FILTER || name.includes(process.env.MEMORY_MCP_BROWSER_FILTER);
const secret = `mem_sk_fixture_${'0123456789abcdef'.repeat(6)}`;
const policy = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'";
const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.svg': 'image/svg+xml' };

const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  const file = resolve(bundle, path.startsWith('/assets/') || path.startsWith('/wasm/') ? `.${path}` : 'index.html');
  if (!file.startsWith(bundle + sep)) {
    response.writeHead(403).end();
    return;
  }
  try {
    let body = await readFile(file);
    if (extname(file) === '.html') body = body.toString().replaceAll('/__memory_mcp_base__', '');
    response.writeHead(200, {
      'Content-Type': types[extname(file)] || 'application/octet-stream',
      'Content-Security-Policy': policy,
      'Cache-Control': 'no-store',
    }).end(body);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.MEMORY_MCP_CHROMIUM_EXECUTABLE || chromium.executablePath(),
}).catch(async (error) => {
  await new Promise((done) => server.close(done));
  throw error;
});

async function fixture(viewport, clipboard = 'real', options = {}) {
  const context = await browser.newContext({
    viewport,
    hasTouch: options.touch ?? viewport.width < 768,
    reducedMotion: options.motion || 'no-preference',
    permissions: ['clipboard-read', 'clipboard-write'],
  });
  if (clipboard !== 'real') {
    await context.addInitScript((mode) => {
      Object.defineProperty(navigator.clipboard, 'writeText', {
        value: (value) => {
          window.clipboardAttempts = (window.clipboardAttempts || 0) + 1;
          window.attemptedClipboardValue = value;
          if (mode === 'refuse') return Promise.reject(new DOMException('Fixture refusal', 'NotAllowedError'));
          return new Promise((resolve) => { window.finishClipboardWrite = resolve; });
        },
      });
    }, clipboard);
  }
  if (options.textScale) {
    await context.route('**/assets/fixture-text-scale.css', (route) => route.fulfill({
      contentType: 'text/css', body: `html { font-size: ${options.textScale}%; }`,
    }));
    await context.addInitScript(() => document.addEventListener('DOMContentLoaded', () => {
      const link = document.createElement('link');
      link.rel = 'stylesheet';
      link.href = '/assets/fixture-text-scale.css';
      document.head.append(link);
    }, { once: true }));
  }
  const page = await context.newPage();
  page.setDefaultTimeout(15_000);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const keys = [
    { id: 'ak_revoked', name: 'old key', status: 'revoked', created_at: '2026-10-02T13:50:00Z', expires_at: null, last_used_at: null },
    { id: 'ak_active', name: 'codex', status: 'active', created_at: '2026-10-05T16:22:00Z', expires_at: null, last_used_at: null },
  ];
  if (options.manyKeys) {
    keys.push(...Array.from({ length: 30 }, (_, index) => ({
      ...keys[1], id: `ak_extra_${index}`, name: `agent ${index}`,
    })));
  }
  const client = {
    account_id: 'acct_fixture', tenant_id: 'ten_fixture',
    display_name: options.longName ? 'a'.repeat(100) : 'Acme',
    account_status: 'active', tenant_status: 'ready',
    plan_version: 1, schema_version: 1, version: 1, provisioning_reason: null,
  };
  const clients = [client, {
    ...client, account_id: 'acct_failed', display_name: 'Migration failed',
    tenant_status: 'failed', provisioning_reason: 'x'.repeat(200),
  }];
  const deletes = [];
  let issued = 0;
  await page.route((url) => url.pathname.startsWith('/api/v1/'), async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (value, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(value) });
    if (path === '/api/v1/auth/config') return json({ methods: ['oidc', 'local'] });
    if (path === '/api/v1/auth/local/csrf') return json({ csrf_token: 'fixture-preauth' });
    if (path === '/api/v1/auth/local/login') return json({ error: { code: 'invalid_credentials', message: 'fixture' } }, 401);
    if (path === '/api/v1/admin/session') return json({
      admin_id: 'admin_fixture', username: 'operator',
      auth_time: '2026-10-08T18:00:00Z', absolute_expiry: '2099-10-08T19:00:00Z',
      csrf_token: 'fixture-session',
    });
    if (path === '/api/v1/admin/clients') return json({ items: clients, next_cursor: null });
    if (path === '/api/v1/admin/clients/acct_fixture') return json(client);
    if (path === '/api/v1/admin/clients/acct_fixture/keys' && request.method() === 'GET') return json({ items: keys, next_cursor: null });
    if (path === '/api/v1/admin/clients/acct_fixture/keys' && request.method() === 'POST') {
      return json({ id: 'ak_created', name: request.postDataJSON().name, secret, expires_at: null }, 201);
    }
    if (path === '/api/v1/account') return json({ account: { id: 'acct_5a22d9a3-b4cb-45a1-81fe-327aacaa9761c', tenant_id: 'ten_850d6061-3e59-4291-a9f0-cc51c2c72cf8', status: 'active', created_at: '2026-10-02T13:47:00Z' } });
    if (path === '/api/v1/account/csrf') return json({ csrf_token: 'fixture-csrf' });
    if (path === '/api/v1/account/api_keys' && request.method() === 'GET') return json(keys);
    if (path === '/api/v1/account/api_keys' && request.method() === 'POST') {
      const { name } = request.postDataJSON();
      const id = `ak_fixture_${++issued}`;
      keys.push({ id, name, status: 'active', created_at: '2026-10-08T18:00:00Z', expires_at: null, last_used_at: null });
      return json({ id, name, secret, expires_at: null }, 201);
    }
    if (request.method() === 'DELETE' && path.startsWith('/api/v1/account/api_keys/')) {
      const id = path.split('/').at(-1);
      deletes.push(id);
      const key = keys.find((key) => key.id === id);
      assert(key && key.status === 'active', 'Only an active fixture key may be revoked');
      key.status = 'revoked';
      return route.fulfill({ status: 204 });
    }
    throw new Error(`Unexpected fixture request: ${request.method()} ${path}`);
  });
  return { context, page, errors, deletes };
}

async function openKeys(page) {
    await page.goto(`${origin}/keys`);
    await page.getByRole('row').filter({ hasText: 'codex' }).waitFor();
    assert(await page.locator('.create-key').evaluate((form) => {
      const input = form.querySelector('input').getBoundingClientRect();
      const submit = form.querySelector('button[type="submit"]').getBoundingClientRect();
      return submit.top - input.bottom <= 16;
    }), 'Create key stays grouped with its input');
}

async function issue(page, name = 'test agent') {
  await page.getByLabel('Key name', { exact: true }).fill(name);
  await page.getByRole('button', { name: 'Create key', exact: true }).click();
  const dialog = page.getByRole('alertdialog', { name: 'API key created', exact: true });
  await dialog.waitFor();
  assert(await dialog.evaluate((element) => {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    const pixel = (color) => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data];
    };
    const base = pixel(getComputedStyle(document.body).backgroundColor);
    const overlay = pixel(getComputedStyle(element).backgroundColor);
    return base.slice(0, 3).every((channel, index) => Math.abs(channel - overlay[index]) <= 2)
      && Math.abs(overlay[3] / 255 - 0.82) < 0.01;
  }), 'The translucent backdrop preserves the console background hue');
  const value = dialog.getByRole('textbox', { name: 'API key', exact: true });
  assert.equal(await value.inputValue(), secret);
  assert(await value.getAttribute('readonly') !== null);
  assert.equal(await dialog.getByRole('checkbox').count(), 0);
  assert(await dialog.getByRole('button', { name: 'Close', exact: true }).isEnabled());
  assert(await dialog.getByRole('button', { name: 'Copy', exact: true }).evaluate((element) => document.activeElement === element));
  assert(await page.locator('.page-surface').evaluate((element) => element.inert));
  return dialog;
}

async function assertFits(page, dialog) {
  const geometry = await page.evaluate(() => ({
    viewport: innerWidth, page: document.documentElement.scrollWidth,
    direction: document.documentElement.dir,
  }));
  assert(geometry.page <= geometry.viewport, `No horizontal page overflow: ${JSON.stringify(geometry)}`);
  if (dialog) {
    assert(await dialog.locator('.secret').evaluate((element) => element.scrollWidth <= element.clientWidth), 'Secret panel fits its viewport');
    assert(await dialog.locator('.secret-copy-row').evaluate((element) => {
      const [input, copy] = element.children;
      const a = input.getBoundingClientRect();
      const b = copy.getBoundingClientRect();
      return Math.abs(a.top - b.top) <= 1
        ? Math.abs(a.height - b.height) <= 1
        : b.top >= a.bottom && b.top - a.bottom <= parseFloat(getComputedStyle(element).rowGap) + 1;
    }), 'Secret field and Copy align or wrap as one group');
  }
}

async function capture(page, name) {
  if (captures) {
    await mkdir(captures, { recursive: true });
    await page.screenshot({ path: resolve(captures, `${name}.png`), fullPage: !name.includes('new-key') && !name.includes('dialog-footer') });
    if (name.endsWith('-new-key')) {
      await page.locator('.secret').screenshot({ path: resolve(captures, `${name}-panel.png`) });
    }
  }
}

async function assertReleased(page) {
  assert.equal(await page.getByRole('alertdialog').count(), 0);
  assert.equal(await page.locator('.secret-value').count(), 0, 'Closing removes the secret from the DOM');
  assert(await page.getByLabel('Key name', { exact: true }).isEditable(), 'Account form becomes interactive');
  assert(await page.evaluate(() => !document.querySelector('.page-surface').inert));
  assert(await page.evaluate(() => localStorage.length === 0 && sessionStorage.length === 0));
  assert(!page.url().includes(secret));
}

async function frame(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

async function textContrast(page) {
  return page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    const pixel = (color) => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data];
    };
    const luminance = (rgba) => {
      const [r, g, b] = rgba.map((value) => {
        const v = value / 255;
        return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (a, b) => {
      const x = luminance(pixel(a));
      const y = luminance(pixel(b));
      return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
    };
    return ['body', '.admin-login > p', '.field > label', 'input', '.hint', '.hint a', 'button[type="submit"]'].map((selector) => {
      const element = document.querySelector(selector);
      const css = getComputedStyle(element);
      let ancestor = element;
      while (ancestor.parentElement && pixel(getComputedStyle(ancestor).backgroundColor)[3] === 0) ancestor = ancestor.parentElement;
      return { selector, ratio: contrast(css.color, getComputedStyle(ancestor).backgroundColor) };
    });
  });
}

async function sharedCssAudit() {
  for (const [name, viewport, options] of [
    ['desktop-css', { width: 1280, height: 900 }, { motion: 'reduce' }],
    ['wide-touch-css', { width: 1280, height: 900 }, { touch: true }],
    ['mobile-css', { width: 320, height: 720 }, {}],
    ['landscape-css', { width: 568, height: 320 }, {}],
    ['text-scale-css', { width: 320, height: 720 }, { textScale: 200 }],
  ]) {
    if (!selected(name)) continue;
    const { context, page, errors } = await fixture(viewport, 'real', { ...options, longName: true, manyKeys: true });
    try {
      await page.goto(`${origin}/login`);
      await page.getByLabel('Username', { exact: true }).waitFor();
      const contrasts = await textContrast(page);
      assert(contrasts.every(({ ratio }) => ratio >= 4.5), `Small text contrast: ${JSON.stringify(contrasts)}`);
      await page.keyboard.press('Tab');
      await page.keyboard.press('Enter');
      assert(await page.locator('main').evaluate((element) => element === document.activeElement && getComputedStyle(element).outlineStyle !== 'none'), 'Skip navigation leaves a visible focus indicator');
      await page.getByLabel('Username', { exact: true }).focus();
      assert(await page.getByLabel('Username', { exact: true }).evaluate((element) => {
        const css = getComputedStyle(element);
        return css.outlineStyle === 'solid' && parseFloat(css.outlineWidth) >= 2 && css.borderRadius === '8px';
      }), 'Keyboard focus preserves the control radius');
      await capture(page, `${name}-login-focus`);
      await page.getByLabel('Username', { exact: true }).fill('operator');
      await page.getByLabel('Password', { exact: true }).fill('fixture password');
      await page.getByRole('button', { name: 'Sign in', exact: true }).click();
      await page.getByRole('alert').waitFor();
      await assertFits(page);
      await capture(page, `${name}-login-error`);

      await page.goto(`${origin}/admin/clients`);
      const clientLink = page.getByRole('link', { name: 'a'.repeat(100), exact: true });
      await clientLink.waitFor();
      assert.equal(await clientLink.getAttribute('title'), 'a'.repeat(100));
      await assertFits(page);
      await capture(page, `${name}-clients`);
      await clientLink.click();
      await page.getByLabel('Expires after', { exact: true }).waitFor();
      await assertFits(page);
      assert(await page.locator('.expiry').evaluate((element) => element.scrollWidth <= element.clientWidth), 'Expiry choices fit at narrow widths');
      await capture(page, `${name}-client-detail`);

      await openKeys(page);
      const submit = page.getByRole('button', { name: 'Create key', exact: true });
      const restingColor = await submit.evaluate((element) => getComputedStyle(element).backgroundColor);
      await submit.hover();
      await frame(page);
      const touch = await page.evaluate(() => matchMedia('(any-pointer: coarse)').matches);
      if (touch) {
        assert.equal(await submit.evaluate((element) => getComputedStyle(element).backgroundColor), restingColor, 'Touch does not latch hover styling');
        const sizes = await page.locator('button, a.button').evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().height));
        assert(sizes.every((size) => size >= 44), 'Touch row actions remain tappable on wide and narrow screens');
      }
      if (options.motion === 'reduce') {
        const normal = await submit.evaluate((element) => getComputedStyle(element).backgroundColor);
        await page.mouse.down();
        assert(await submit.evaluate((element) => getComputedStyle(element).transform === 'none' && getComputedStyle(element).transitionDuration === '0s'), 'Reduced motion has no moving transition');
        assert.notEqual(await submit.evaluate((element) => getComputedStyle(element).backgroundColor), normal, 'Press feedback survives reduced motion');
        await page.mouse.move(0, 0);
        await page.mouse.up();
      }
      const before = await page.locator('.create-key').boundingBox();
      const dialog = await issue(page, 'a'.repeat(100));
      const after = await page.locator('.create-key').boundingBox();
      assert(Math.abs(before.x - after.x) <= 1 && Math.abs(before.width - after.width) <= 1, 'Scroll locking does not shift the page gutter');
      assert(await page.evaluate(() => getComputedStyle(document.documentElement).overflow === 'clip'), 'An open modal locks document scrolling');
      const scroll = await page.evaluate(() => scrollY);
      await page.mouse.move(4, 60);
      await page.mouse.wheel(0, 500);
      await frame(page);
      assert.equal(await page.evaluate(() => scrollY), scroll, 'The page behind a modal cannot wheel-scroll');
      await assertFits(page, dialog);
      assert(await dialog.locator('.secret').evaluate((element) => {
        const box = element.getBoundingClientRect();
        return box.top >= 15 && box.bottom <= innerHeight - 15;
      }), 'A dialog stays inside a short viewport');
      await capture(page, `${name}-new-key`);
      const close = dialog.getByRole('button', { name: 'Close', exact: true });
      await close.scrollIntoViewIfNeeded();
      assert(await close.evaluate((element) => {
        const box = element.getBoundingClientRect();
        return box.top >= 0 && box.bottom <= innerHeight;
      }), 'The exit remains reachable when the modal body scrolls');
      await capture(page, `${name}-dialog-footer`);
      await close.click();
      await assertReleased(page);
      await assertFits(page);
      assert(await page.evaluate(() => getComputedStyle(document.documentElement).overflow !== 'clip'), 'Closing restores document scrolling');

      await page.evaluate(() => { document.documentElement.dir = 'rtl'; });
      const table = page.getByRole('region', { name: 'API keys table' });
      await table.evaluate((element) => { element.scrollLeft = -element.scrollWidth; });
      await capture(page, `${name}-rtl`);
      assert(await table.evaluate((element) => {
        const cell = element.querySelector('tbody tr td:first-child').getBoundingClientRect();
        const box = element.getBoundingClientRect();
        return Math.abs(cell.right - box.right) <= 2;
      }), 'The sticky identity column follows the inline start in RTL');
      await assertFits(page);
      assert.deepEqual(errors, []);
      console.log(`${name}: focus, touch, reduced motion, overflow, scroll lock, short dialog and RTL passed`);
    } finally {
      await context.close();
    }
  }
}

try {
  for (const [name, viewport] of [
    ['desktop', { width: 1280, height: 900 }],
    ['tablet', { width: 768, height: 900 }],
    ['mobile', { width: 390, height: 844 }],
    ['small-mobile', { width: 320, height: 720 }],
  ]) {
    if (!selected(name)) continue;
    const { context, page, errors, deletes } = await fixture(viewport);
    try {
      await page.goto(`${origin}/`);
      const metadata = page.getByRole('region', { name: 'Account metadata table' });
      await metadata.waitFor();
      assert(await metadata.evaluate((element) => parseFloat(getComputedStyle(element).borderRadius) > 0 && getComputedStyle(element).overflowX === 'auto'));
      await assertFits(page);
      await capture(page, `${name}-status`);
      await openKeys(page);
      const revoked = page.getByRole('row').filter({ hasText: 'old key' }).getByRole('button', { name: 'Revoke…', exact: true });
      assert(await revoked.isDisabled(), 'A revoked key cannot be revoked again');
      await revoked.evaluate((element) => element.click());
      assert.equal(await page.locator('.confirm-row').count(), 0);
      assert.deepEqual(deletes, []);
      const activeRow = page.getByRole('row').filter({ hasText: 'codex' });
      assert(await activeRow.getByRole('button').isEnabled());
      await capture(page, `${name}-keys`);
      const dialog = await issue(page, name === 'small-mobile' ? 'a'.repeat(120) : 'test agent');
      await assertFits(page, dialog);
      await capture(page, `${name}-new-key`);
      await dialog.getByRole('button', { name: 'Copy', exact: true }).click();
      await dialog.getByRole('button', { name: 'Copied', exact: true }).waitFor();
      assert(await dialog.evaluate((element) => element.contains(document.activeElement)), 'Copy keeps keyboard focus inside the dialog');
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), secret);
      assert(await dialog.isVisible(), 'Copy leaves the key visible');
      await dialog.getByRole('button', { name: 'Copy and close', exact: true }).click();
      await dialog.waitFor({ state: 'hidden' });
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), secret);
      await assertReleased(page);
      await activeRow.getByRole('button').click();
      await Promise.all([
        page.waitForResponse((response) => response.request().method() === 'DELETE' && response.url().endsWith('/ak_active')),
        page.getByRole('button', { name: 'Confirm revoke', exact: true }).click(),
      ]);
      await page.waitForFunction(() => [...document.querySelectorAll('tbody tr')].some((row) => row.textContent.includes('codex') && /revoked/i.test(row.textContent) && row.querySelector('button')?.disabled));
      assert.deepEqual(deletes, ['ak_active']);
      const manuallySaved = await issue(page);
      await manuallySaved.getByRole('button', { name: 'Close', exact: true }).click();
      await manuallySaved.waitFor({ state: 'hidden' });
      await assertReleased(page);
      const escaped = await issue(page);
      await page.keyboard.press('Escape');
      await escaped.waitFor({ state: 'hidden' });
      await assertReleased(page);
      assert.deepEqual(errors, []);
      console.log(`${name}: rounding, revoked guard, copy, copy-and-close, manual close, Escape, layout passed`);
    } finally {
      await context.close();
    }
  }

  for (const clipboard of ['refuse', 'defer']) {
    if (!selected(`clipboard-${clipboard}`)) continue;
    const { context, page, errors } = await fixture({ width: 390, height: 844 }, clipboard);
    try {
      await openKeys(page);
      const dialog = await issue(page);
      await dialog.getByRole('button', { name: 'Copy and close', exact: true }).click();
      if (clipboard === 'refuse') {
        await dialog.getByRole('alert').waitFor();
        assert.match(await dialog.getByRole('alert').innerText(), /select.*copy/i);
        assert(await dialog.isVisible(), 'A failed write never closes the panel');
        assert.equal(await dialog.getByRole('textbox').inputValue(), secret);
        await capture(page, 'mobile-clipboard-refused');
        await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      } else {
        await dialog.getByRole('button', { name: 'Copying…', exact: true }).waitFor();
        assert(await dialog.getByRole('button', { name: 'Copying…', exact: true }).isDisabled());
        assert(await dialog.getByRole('button', { name: 'Copy and close', exact: true }).isDisabled());
        assert(await dialog.isVisible(), 'Copy and close waits for clipboard completion');
        assert(await dialog.evaluate((element) => element.contains(document.activeElement)), 'Pending copy keeps Escape reachable');
        await dialog.getByRole('button', { name: 'Copy and close', exact: true }).evaluate((element) => element.click());
        assert.equal(await page.evaluate(() => window.clipboardAttempts), 1, 'An in-flight copy is not submitted twice');
        assert.equal(await page.evaluate(() => window.attemptedClipboardValue), secret);
        await page.evaluate(() => window.finishClipboardWrite());
      }
      await dialog.waitFor({ state: 'hidden' });
      await assertReleased(page);
      assert.deepEqual(errors, []);
      console.log(`clipboard ${clipboard}: recovery and completion ordering passed`);
    } finally {
      await context.close();
    }
  }
  await sharedCssAudit();
} finally {
  await browser.close();
  await new Promise((done) => server.close(done));
}
