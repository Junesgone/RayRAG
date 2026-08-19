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
await page.reload({ waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2500);
const info = await page.evaluate(() => {
  const uses = Array.from(document.querySelectorAll('use'));
  const rendered = uses.filter(u => u.ownerSVGElement && u.ownerSVGElement.querySelector('path')).length;
  const bad = uses.filter(u => /\+entry/.test(u.getAttribute('href')||'')).length;
  return { useCount: uses.length, rendered, bad, sample: uses.slice(0,5).map(u=>u.getAttribute('href')) };
});
console.log(JSON.stringify(info, null, 1));
await page.screenshot({ path: 'providers-final.png', fullPage: false });
await page.close();
await browser.close();
