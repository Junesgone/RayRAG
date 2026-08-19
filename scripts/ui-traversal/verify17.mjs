import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1400, height: 2000 });
await page.goto('http://127.0.0.1:9390/login');
await page.evaluate(async () => {
  const r = await fetch('/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password:'rayrag-smoke-admin'})});
  const j = await r.json();
  localStorage.setItem('rayrag_token', j.data.access_token);
  document.cookie = 'rayrag_token='+j.data.access_token+';path=/;max-age=86400';
});
await page.goto('http://127.0.0.1:9390/user-setting/model?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
const boxes = await page.evaluate(() => {
  const els = Array.from(document.querySelectorAll('.provider-brand-icon')).slice(0, 24);
  return els.map(el => { const r = el.getBoundingClientRect(); const u = el.querySelector && el.querySelector('use'); return { x: r.x, y: r.y, w: r.width, h: r.height, href: u ? u.getAttribute('href') : (el.getAttribute ? el.getAttribute('src') : null) };
    });
});
console.log(JSON.stringify(boxes, null, 0));
await page.screenshot({ path: 'icon-px2.png' });
await page.close();
await browser.close();
