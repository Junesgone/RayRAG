import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = ctx.pages().find(p => p.url().includes('9390')) || (await ctx.newPage());
const reqs = [], errors = [];
page.on('request', r => { if (r.url().includes('/data_sources/')) reqs.push(r.method() + ' ' + r.url().replace('http://127.0.0.1:9390', '')); });
page.on('pageerror', e => errors.push(String(e).slice(0, 80)));
const tok = (await (await fetch('http://127.0.0.1:9390/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password: 'rayrag-smoke-admin' }) })).json()).data.access_token;
await page.goto('http://127.0.0.1:9390/login', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(() => {});
await page.evaluate(t => localStorage.setItem('rayrag_token', t), tok);
await page.goto('http://127.0.0.1:9390/user-setting/data-source', { waitUntil: 'networkidle', timeout: 45000 }).catch(() => {});
await page.waitForTimeout(7000);
await page.locator('.ds-added-card', { hasText: 'probe-local-src' }).first().locator('button[title="Settings"]').click({ timeout: 10000 }).catch(() => {});
await page.waitForTimeout(3500);
console.log('detail path:', await page.evaluate(() => location.pathname));
reqs.length = 0;
const testBtn = page.locator('button:has-text("Test")').first();
console.log('Test visible:', await testBtn.isVisible().catch(() => false));
await testBtn.click({ timeout: 10000 }).catch(e => console.log('click err', String(e).slice(0, 70)));
await page.waitForTimeout(4000);
const after = await page.evaluate(() => {
  const texts = [...document.querySelectorAll('[id*=ds],[class*=ds-status],[class*=toast],[class*=notice]')]
    .filter(e => e.offsetParent !== null && (e.textContent || '').trim())
    .map(e => ({ id: e.id || e.className.slice(0, 22), text: (e.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 70) })).slice(0, 6);
  return { statusEls: texts, bodyMentionsResult: /verified|connected|disconnected|failed|成功|失败/i.test(document.body.innerText) };
});
console.log('after Test:', JSON.stringify(after));
console.log('requests:', JSON.stringify(reqs));
console.log('pageerrors:', JSON.stringify(errors));
await b.close();
