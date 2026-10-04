import { chromium } from 'playwright-core';
const SRC = 'a978fdf0-3225-46bc-9e2c-5dc667f2c6bf';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = ctx.pages().find(p => p.url().includes('9390')) || (await ctx.newPage());
const errors = [], reqs = [];
page.on('pageerror', e => errors.push(String(e).slice(0, 120)));
page.on('response', r => { if (r.url().endsWith('/test')) reqs.push(r.status() + ' ' + (r.headers()['content-type'] || '')); });
const tok = (await (await fetch('http://127.0.0.1:9390/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password: 'rayrag-smoke-admin' }) })).json()).data.access_token;
await page.goto('http://127.0.0.1:9390/login', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(() => {});
await page.evaluate(t => localStorage.setItem('rayrag_token', t), tok).catch(() => {});
await page.context().addCookies([{ name: 'lang', value: 'zh-CN', url: 'http://127.0.0.1:9390' }]).catch(() => {});
await page.goto('http://127.0.0.1:9390/user-setting/data-source/' + SRC, { waitUntil: 'networkidle', timeout: 45000 }).catch(() => {});
await page.waitForTimeout(9000);
console.log('path:', await page.evaluate(() => location.pathname).catch(() => 'n/a'));
const btn = page.locator('#dsDetailTest');
console.log('Test visible:', await btn.isVisible().catch(() => false), '| label:', (await btn.textContent().catch(() => ''))?.trim());
await btn.click({ timeout: 10000 }).catch(e => console.log('click err', String(e).slice(0, 60)));
await page.waitForTimeout(6000);
const after = await page.evaluate(() => {
  const s = document.getElementById('dsDetailStatus');
  const rows = [...document.querySelectorAll('table tbody tr')];
  return {
    status: s ? s.textContent.trim().slice(0, 120) : null,
    statusColor: s ? s.style.color : null,
    firstLogRow: rows.length ? (rows[0].innerText || '').replace(/\s+/g, ' ').slice(0, 110) : null,
    logRows: rows.length,
  };
}).catch(e => ({ evalError: String(e).slice(0, 80) }));
console.log('after Test:', JSON.stringify(after));
console.log('responses:', JSON.stringify(reqs));
console.log('pageerrors:', JSON.stringify(errors));
await b.close();
