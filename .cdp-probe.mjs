import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = ctx.pages().find(p => p.url().includes('9390')) || (await ctx.newPage());
const tok = (await (await fetch('http://127.0.0.1:9390/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password: 'rayrag-smoke-admin' }) })).json()).data.access_token;
await page.goto('http://127.0.0.1:9390/login', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(() => {});
await page.evaluate(t => localStorage.setItem('rayrag_token', t), tok);
await page.context().addCookies([{ name: 'lang', value: 'zh-CN', url: 'http://127.0.0.1:9390' }]).catch(() => {});
await page.goto('http://127.0.0.1:9390/files', { waitUntil: 'networkidle', timeout: 45000 }).catch(() => {});
await page.waitForTimeout(8000);
const before = await page.evaluate(() => ({
  path: location.pathname,
  hasHost: !!document.getElementById('filePagination'),
  hostHTML: (document.getElementById('filePagination') || {}).innerHTML?.slice(0, 60) || '',
  rows: document.querySelectorAll('#fl tr, table tr').length,
}));
console.log('BEFORE', JSON.stringify(before));
const after = await page.evaluate(() => {
  const fn = window.renderFilePagination;
  if (typeof fn !== 'function') return { callable: false };
  fn(2);
  const host = document.getElementById('filePagination');
  const sel = document.querySelector('.list-page-size select');
  return { callable: true, hostHTML: host.innerHTML.slice(0, 120), options: sel ? [...sel.options].map(o => o.textContent.trim()) : null, total: (host.innerText || '').trim() };
});
console.log('AFTER', JSON.stringify(after));
await b.close();
