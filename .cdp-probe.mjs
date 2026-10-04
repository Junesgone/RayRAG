import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = ctx.pages().find(p => p.url().includes('9390')) || (await ctx.newPage());
const reqs = [], errors = [];
page.on('request', r => { if (/\/data_sources\/[^/]+\/(test|files|disconnect)/.test(r.url())) reqs.push(r.method() + ' ' + r.url().split('/api/v1')[1]); });
page.on('pageerror', e => errors.push(String(e).slice(0, 80)));
const tok = (await (await fetch('http://127.0.0.1:9390/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password: 'rayrag-smoke-admin' }) })).json()).data.access_token;
await page.goto('http://127.0.0.1:9390/login', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(() => {});
await page.evaluate(t => localStorage.setItem('rayrag_token', t), tok);
await page.context().addCookies([{ name: 'lang', value: 'zh-CN', url: 'http://127.0.0.1:9390' }]).catch(() => {});
await page.goto('http://127.0.0.1:9390/user-setting/data-source', { waitUntil: 'networkidle', timeout: 45000 }).catch(() => {});
await page.waitForTimeout(7000);
await page.locator('.ds-added-card', { hasText: 'probe-local-src' }).first().locator('button[title="Settings"]').click({ timeout: 10000 }).catch(() => {});
await page.waitForTimeout(3500);
console.log('path:', await page.evaluate(() => location.pathname));
for (const id of ['dsDetailTest', 'dsDetailSync', 'dsDetailDisconnect']) {
  const loc = page.locator(`#${id}`);
  const visible = await loc.isVisible().catch(() => false);
  const label = visible ? (await loc.textContent())?.trim() : null;
  if (!visible) { console.log(`${id}: NOT VISIBLE`); continue; }
  await loc.click({ timeout: 10000 }).catch(e => console.log(`  click err ${String(e).slice(0, 50)}`));
  await page.waitForTimeout(4000);
  const status = await page.evaluate(() => { const s = document.getElementById('dsDetailStatus'); return s ? s.textContent.trim().slice(0, 70) : null; });
  console.log(`${id} [${label}] -> status=${JSON.stringify(status)}`);
}
console.log('requests:', JSON.stringify(reqs));
console.log('pageerrors:', JSON.stringify(errors));
await b.close();
