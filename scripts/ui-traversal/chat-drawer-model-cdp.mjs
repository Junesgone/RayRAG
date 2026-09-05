import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || 'http://127.0.0.1:9390';
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error('RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)');
  process.exit(2);
}
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = {};
let appId = null;
try {
  await ctx.addCookies([{ name: 'lang', value: 'zh-CN', domain: '127.0.0.1', path: '/' }]);
  await page.goto(`${ORIGIN}/login`, { waitUntil: 'domcontentloaded', timeout: 40000 });
  await page.evaluate(async ({ password }) => {
    const r = await fetch('/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password }) });
    const j = await r.json();
    localStorage.setItem('rayrag_token', j.data.access_token);
    document.cookie = 'rayrag_token=' + j.data.access_token + ';path=/;max-age=86400';
  }, { password: PASSWORD });
  mkdirSync('docs/ui-traversal/rayrag-audit', { recursive: true });
  const created = await page.evaluate(async () => {
    const r = await fetch('/api/v1/chatapps', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name: 'CC-Probe-Drawer-' + Date.now() }) });
    const j = await r.json();
    return j.data || null;
  });
  results.created = created ? created.id || null : null;
  appId = results.created;
  if (!appId) throw new Error('chat app create failed');
  await page.goto(`${ORIGIN}/chats/${appId}`, { waitUntil: 'domcontentloaded', timeout: 40000 });
  await page.waitForTimeout(1500);
  await page.locator("[onclick='toggleSettings()']").first().click({ timeout: 8000 });
  await page.waitForTimeout(900);
  results.drawerOpened = await page.evaluate(() => document.getElementById('settingsDrawer').classList.contains('open'));
  const seed = await page.evaluate(() => {
    const trig = document.getElementById('dmLlm');
    return { value: trig.dataset.value || '', saved: trig.dataset.savedValue || '', display: document.getElementById('dmLlmValue').textContent, state: trig.getAttribute('data-state') };
  });
  results.seed = seed;
  await page.screenshot({ path: 'docs/ui-traversal/rayrag-audit/chat-drawer-seed-20260904.png' });
  await page.locator("#dmLlm").click({ timeout: 8000 });
  await page.waitForTimeout(900);
  const opened = await page.evaluate(() => {
    const trig = document.getElementById('dmLlm');
    const rows = Array.from(document.querySelectorAll('#dmLlmTree .default-model-node'));
    return { state: trig.getAttribute('data-state'), rows: rows.length, leaves: document.querySelectorAll('#dmLlmTree .default-model-leaf').length };
  });
  results.treeOpened = opened;
  const leaf = page.locator("#dmLlmTree .default-model-leaf").last();
  const leafText = await leaf.textContent();
  await leaf.click();
  await page.waitForTimeout(700);
  const picked = await page.evaluate(() => {
    const trig = document.getElementById('dmLlm');
    return { value: trig.dataset.value || '', state: trig.getAttribute('data-state') };
  });
  results.picked = picked;
  await page.screenshot({ path: 'docs/ui-traversal/rayrag-audit/chat-drawer-picked-20260904.png' });
  await page.locator("[onclick='saveSettings()']").first().click({ timeout: 8000 });
  await page.waitForTimeout(1800);
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 40000 });
  await page.waitForTimeout(1500);
  await page.locator("[onclick='toggleSettings()']").first().click({ timeout: 8000 });
  await page.waitForTimeout(900);
  const after = await page.evaluate(() => {
    const trig = document.getElementById('dmLlm');
    return { value: trig.dataset.value || '', saved: trig.dataset.savedValue || '', display: document.getElementById('dmLlmValue').textContent };
  });
  results.after = after;
  await page.screenshot({ path: 'docs/ui-traversal/rayrag-audit/chat-drawer-after-20260904.png' });
  results.result = results.drawerOpened && seed.value !== '' && opened.rows > 0 && picked.value !== '' && picked.value === leafText.trim() && after.value === picked.value && after.saved === picked.value ? 'PASS' : 'FAIL';
} catch (e) {
  results.error = String(e && e.message ? e.message : e);
  results.result = 'FAIL';
} finally {
  try {
    if (appId) {
      await page.evaluate(async (id) => { await fetch('/api/v1/chatapps/' + id, { method: 'DELETE' }); }, appId);
      results.cleaned = true;
    }
  } catch (e) { results.cleaned = 'err:' + String(e); }
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await browser.close();
process.exit(results.result && results.result.startsWith('PASS') ? 0 : 1);
