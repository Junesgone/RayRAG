import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/login');
await page.evaluate(async () => {
  const r = await fetch('/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password:'rayrag-smoke-admin'})});
  const j = await r.json();
  localStorage.setItem('rayrag_token', j.data.access_token);
  document.cookie = 'rayrag_token='+j.data.access_token+';path=/;max-age=86400';
});
for (const route of ['/datasets','/chats','/searches','/agents','/memories','/files']) {
  await page.goto('http://127.0.0.1:9390' + route + '?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(900);
  const btns = await page.evaluate(() => Array.from(document.querySelectorAll('button,a')).map(b => b.textContent.trim()).filter(t => t && t.length < 24));
  console.log(route, '=>', JSON.stringify([...new Set(btns)].slice(0, 16)));
}
await page.close();
await browser.close();
