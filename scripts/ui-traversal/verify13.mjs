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
await page.goto('http://127.0.0.1:9390/user-setting/model?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
const info = await page.evaluate(() => {
  const uses = Array.from(document.querySelectorAll('use'));
  let rendered = 0;
  for (const u of uses) {
    const sr = u.shadowRoot;
    if (sr && sr.querySelector('path,rect,circle')) rendered++;
  }
  return { useCount: uses.length, renderedWithContent: rendered };
});
console.log(JSON.stringify(info, null, 1));
await page.screenshot({ path: 'providers-final4.png' });
await page.close();
await browser.close();
