// Verify RayRAG's /compilation-templates/edit-next page in a real browser (CDP).
// Creates a temporary template group, checks create + edit modes, then deletes it.
import { chromium } from 'playwright-core';
import { readFileSync } from 'node:fs';

const CDP = process.env.CDP_URL || 'http://127.0.0.1:16002';
const ORIGIN = process.env.RAYRAG_ORIGIN || 'http://127.0.0.1:9391';
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

const password = process.env.RAYRAG_ADMIN_PASSWORD || readEnv(PROJECT + '/.env').RAYRAG_ADMIN_PASSWORD;
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
page.on('dialog', (dialog) => dialog.accept());

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

  await page.goto(ORIGIN + '/compilation-templates/edit-next', { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.waitForFunction(() => document.querySelectorAll('#ctKind option').length > 1, { timeout: 30000 }).catch(() => fail('kind options did not load'));
  const title = (await page.textContent('#ctTitle') || '').trim();
  if (title !== 'Add template') fail('title expected "Add template", got "' + title + '"');
  for (const selector of ['#ctName', '#ctDescription', '#ctKind', '#ctGlobalRules', '#ctRechunk', '#ctSaveButton']) {
    if (!(await page.$(selector))) fail('missing field ' + selector);
  }
  const kinds = await page.$$eval('#ctKind option', (options) => options.map((option) => option.value).filter(Boolean));
  ok('kind options: ' + kinds.join(','));
  const kind = kinds.includes('knowledge_graph') ? 'knowledge_graph' : kinds[0];
  await page.selectOption('#ctKind', kind).catch((error) => fail('select kind: ' + String(error)));
  await page.waitForTimeout(400);
  const name = 'cdp-template-' + Date.now();
  await page.fill('#ctName', name);
  await page.fill('#ctDescription', 'Cdp verification');
  await page.fill('#ctGlobalRules', 'Rule from CDP');
  await page.check('#ctRechunk');
  if (!(await page.isVisible('#ctRechunkRulesField'))) fail('rechunk rules should appear when re-chunk is checked');
  await page.fill('#ctRechunkRules', 'Rules from CDP');
  await page.click('#ctSaveButton');
  await page.waitForURL('**/agents', { timeout: 30000 }).catch(() => fail('save did not navigate to /agents'));
  if (!page.url().includes('/agents')) fail('expected /agents after save, at ' + page.url());
  else ok('save navigated to /agents');

  const groups = await page.evaluate(async () => (await fetch('/api/v1/compilation_template_groups')).json());
  const group = (groups.data && groups.data.groups || []).find((item) => item.name === name);
  if (!group) fail('created group not found: ' + name);
  else {
    const template = (group.templates && group.templates[0]) || {};
    const config = template.config || {};
    if (template.kind !== kind) fail('kind mismatch: ' + template.kind);
    if (config.global_rules !== 'Rule from CDP') fail('global_rules mismatch: ' + JSON.stringify(config.global_rules));
    if (config.rechunk !== true) fail('rechunk mismatch: ' + JSON.stringify(config.rechunk));
    if (config.rechunk_rules !== 'Rules from CDP') fail('rechunk_rules mismatch: ' + JSON.stringify(config.rechunk_rules));
    ok('created group ' + group.id + ' with kind/global_rules/rechunk persisted');

    await page.goto(ORIGIN + '/compilation-templates/edit-next/' + group.id, { waitUntil: 'domcontentloaded', timeout: 60000 });
    await page.waitForFunction(() => (document.querySelector('#ctName') || {}).value, { timeout: 30000 }).catch(() => fail('edit page did not populate'));
    const editName = await page.inputValue('#ctName');
    const editKind = await page.inputValue('#ctKind');
    const editRules = await page.inputValue('#ctGlobalRules');
    const editTitle = (await page.textContent('#ctTitle') || '').trim();
    if (editName !== name) fail('edit name mismatch: ' + editName);
    if (editKind !== kind) fail('edit kind mismatch: ' + editKind);
    if (editRules !== 'Rule from CDP') fail('edit global rules mismatch: ' + editRules);
    if (editTitle !== name) fail('edit title expected group name, got: ' + editTitle);
    if (!failures.length) ok('edit mode populated from the API');

    await page.evaluate(async (id) => {
      await fetch('/api/v1/compilation_template_groups/' + encodeURIComponent(id), { method: 'DELETE' });
    }, group.id);
    const after = await page.evaluate(async () => (await fetch('/api/v1/compilation_template_groups')).json());
    const stillThere = (after.data && after.data.groups || []).some((item) => item.name === name);
    if (stillThere) fail('cleanup failed');
    else ok('cleanup deleted the temporary group');
  }
} catch (error) {
  fail('unexpected: ' + String(error));
}

// Agents page entry: section + Add navigation + card delete.
  await page.goto(ORIGIN + '/agents', { waitUntil: 'domcontentloaded', timeout: 60000 });
  if (!(await page.$("[data-testid='agent-compilation-section']"))) fail('agents page misses the compilation section');
  if (!(await page.$("[data-testid='agent-compilation-add']"))) fail('agents page misses the Add template button');
  await page.click("[data-testid='agent-compilation-add']").catch(() => fail('Add template click failed'));
  await page.waitForURL('**/compilation-templates/edit-next', { timeout: 20000 }).catch(() => fail('Add did not navigate to edit-next'));
  if (!page.url().includes('/compilation-templates/edit-next')) fail('Add landed on ' + page.url());
  const entryName = 'cdp-entry-' + Date.now();
  const created = await page.evaluate(async (name) => {
    const response = await fetch('/api/v1/compilation_template_groups', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name, description: 'entry check', templates: [{ name, description: 'entry check', kind: 'knowledge_graph', config: { kind: 'knowledge_graph', global_rules: '', rechunk: false, rechunk_rules: '' } }] }),
    });
    return response.json();
  }, entryName);
  const entryId = created && created.data && created.data.id;
  if (!entryId) fail('could not create the entry-check group');
  else {
    await page.goto(ORIGIN + '/agents', { waitUntil: 'domcontentloaded', timeout: 60000 });
    await page
      .waitForFunction((id) => document.querySelector("[data-testid='agent-compilation-card'][data-id='" + id + "']"), entryId, { timeout: 20000 })
      .catch(() => fail('compilation card did not render for ' + entryId));
    const card = await page.$("[data-testid='agent-compilation-card'][data-id='" + entryId + "']");
    if (!card) fail('compilation card missing on /agents');
    else {
      await card.$eval('.acg-delete', (node) => node.click()).catch(() => fail('card delete click failed'));
      await page.waitForTimeout(1500);
      const gone = await page.evaluate(async (id) => {
        const response = await fetch('/api/v1/compilation_template_groups');
        const data = await response.json();
        return !((data.data && data.data.groups) || []).some((item) => item.id === id);
      }, entryId);
      if (!gone) fail('entry-check group still present after delete');
      else ok('agents entry: Add navigation + card render + delete verified');
    }
  }

console.log(failures.length ? 'RESULT FAIL (' + failures.length + ')' : 'RESULT PASS');
await page.close().catch(() => {});
await browser.close();
process.exit(failures.length ? 1 : 0);
