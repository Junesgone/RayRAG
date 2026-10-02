// Verify the v0.27.2 Artifacts tab (/dataset/compilation/{id}) on RayRAG (CDP).
import { chromium } from 'playwright-core';
import { readFileSync } from 'node:fs';

const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || process.env.RAYRAG_ORIGIN || 'http://127.0.0.1:9391';
const PROJECT = '/home/junesgone/.openclaw/workspace/RayRAG';

function readEnv(path) {
  const out = {};
  try {
    for (const line of readFileSync(path, 'utf8').split('\n')) {
      const at = line.indexOf('=');
      if (at > 0) out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
    }
  } catch (error) {
    console.error('cannot read ' + path + ': ' + String(error));
  }
  return out;
}

const password = process.env.RAYRAG_LOGIN_PASSWORD || process.env.RAYRAG_ADMIN_PASSWORD || readEnv(PROJECT + '/.env').RAYRAG_ADMIN_PASSWORD;
if (!password) {
  console.error('no admin password available');
  process.exit(2);
}

const failures = [];
const fail = (message) => { failures.push(message); console.error('FAIL ' + message); };
const ok = (message) => console.log('ok - ' + message);
const browser = await chromium.connectOverCDP(CDP);
const context = browser.contexts()[0] || (await browser.newContext());
const page = await context.newPage();
await page.setViewportSize({ width: 1600, height: 900 }).catch(() => {});

let createdId = null;
try {
  await page.goto(ORIGIN + '/login', { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.evaluate(async (secret) => {
    const response = await fetch('/api/v1/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ email: 'admin@rayrag.local', password: secret }),
    });
    const data = await response.json();
    if (data.code !== 0) throw new Error('login failed');
    localStorage.setItem('rayrag_token', data.data.access_token);
    document.cookie = 'rayrag_token=' + data.data.access_token + ';path=/';
  }, password);

  let kbId = await page.evaluate(async () => {
    const response = await fetch('/api/v1/datasets');
    const data = await response.json();
    const list = (data.data && (data.data.datasets || data.data)) || [];
    return list.length ? list[0].id : '';
  });
  if (!kbId) {
    const name = 'cdp-artifacts-' + Date.now();
    const created = await page.evaluate(async (kbName) => {
      const response = await fetch('/api/v1/datasets', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: kbName, description: 'artifacts tab probe' }),
      });
      return response.json();
    }, name);
    kbId = created && created.data && created.data.id;
    createdId = kbId;
    if (!kbId) fail('could not create a probe dataset');
  }
  if (!kbId) throw new Error('no dataset available');

  await page.goto(ORIGIN + '/dataset/files/' + kbId, { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.waitForSelector("[id='nav5']", { timeout: 20000 }).catch(() => fail('sidebar Artifacts entry missing'));
  const navText = (await page.textContent("[id='nav5']") || '').trim();
  if (navText !== 'Artifacts') fail('sidebar label expected Artifacts, got ' + navText);
  const navHref = await page.getAttribute("[id='nav5']", 'href');
  if (navHref !== '/dataset/compilation/' + kbId) fail('sidebar href mismatch: ' + navHref);
  if (await page.$("[id='nav4']")) fail('knowledge-graph must no longer be a sidebar item');
  ok('sidebar shows Artifacts and hides knowledge-graph');

  await page.click("[id='nav5']");
  await page.waitForTimeout(800);
  if (!page.url().includes('/dataset/compilation/')) fail('Artifacts click did not update the URL: ' + page.url());
  const visible = await page.isVisible("[data-testid='dataset-compilation-panel']");
  if (!visible) fail('Artifacts panel is not visible after the click');
  const options = await page.$$eval('#cwpMode option', (nodes) => nodes.map((node) => node.value));
  if (options.join(',') !== 'llm-wiki,tree,graph,mindmap,timeline') fail('view-mode options mismatch: ' + options.join(','));
  const emptyText = (await page.textContent("[data-testid='dataset-compilation-empty']") || '').trim();
  if (emptyText !== 'No wiki yet') fail('empty state expected "No wiki yet", got ' + emptyText);
  const disabled = await page.getAttribute('#cwpGenerate', 'disabled');
  if (disabled === null) fail('Generate must stay disabled without chunks + pipeline');
  ok('Artifacts panel: view modes, empty state and eligibility gate');

  await page.click("[data-testid='dataset-compilation-back']");
  await page.waitForTimeout(500);
  if (!page.url().includes('/dataset/files/' + kbId)) fail('Back did not return to files: ' + page.url());
  else ok('Back returns to the files tab');
} catch (error) {
  fail('unexpected: ' + String(error));
} finally {
  if (createdId) {
    await page.evaluate(async (id) => {
      await fetch('/api/v1/datasets', { method: 'DELETE', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ids: [id] }) });
    }, createdId).catch(() => {});
  }
}

console.log(failures.length ? 'RESULT FAIL (' + failures.length + ')' : 'RESULT PASS');
await page.close().catch(() => {});
await browser.close();
process.exit(failures.length ? 1 : 0);
