#!/usr/bin/env node
/**
 * Local admin browser acceptance test runner.
 *
 * Usage:
 *   node scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario auth
 *
 * Scenarios:
 *   auth      — Login/activation/reset flow
 *   clients   — Client management flow
 *   regression — Regression tests
 *
 * Environment:
 *   LOCAL_ADMIN_BROWSER_FIXTURE — Path to fixture config file
 */

import { parseArgs } from 'node:util';

const { values } = parseArgs({
  options: {
    'base-url': { type: 'string', default: 'https://localhost:8443' },
    scenario: { type: 'string', default: 'auth' },
  },
  strict: false,
});

const BASE_URL = values['base-url'];
const SCENARIO = values.scenario;

console.log(`Local Admin Browser Runner`);
console.log(`Base URL: ${BASE_URL}`);
console.log(`Scenario: ${SCENARIO}`);
console.log('');

// Placeholder — actual browser tests require a browser package
// (playwright, puppeteer, etc.) which needs separate dependency approval
console.log('Browser test runner requires a browser package (playwright/puppeteer).');
console.log('This is a placeholder for the acceptance test harness.');
console.log('');
console.log('To run actual browser tests:');
console.log('1. Install playwright: npm install playwright');
console.log('2. Set LOCAL_ADMIN_BROWSER_FIXTURE to your fixture config');
console.log('3. Run: node scripts/ci/local_admin_browser.mjs --scenario auth');
console.log('');

// Exit with success for now
process.exit(0);
