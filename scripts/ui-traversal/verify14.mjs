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
  // draw the svg to canvas to check pixels
  const svg = document.querySelector('span.added-model-provider-icon svg');
  const use = svg && svg.querySelector('use');
  const res = {};
  try { res.getNumberOfItems = use.getNumberOfItems(); } catch(e) {}
  try { res.shadowItems = use.shadowRoot ? use.shadowRoot.children.length : null; } catch(e) {}
  // check the available-provider (SSR) icons too
  const svg2 = document.querySelector('span.available-provider-icon svg');
  const use2 = svg2 && svg2.querySelector('use');
  try { res.getNumberOfItems2 = use2.getNumberOfItems(); } catch(e) {}
  // check if the external doc loaded
  return res;
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
