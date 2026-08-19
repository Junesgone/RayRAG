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
  const list = document.getElementById('addedModelList');
  const cards = list ? list.querySelectorAll('[data-testid="added-model-card"]').length : -1;
  const icon = list && list.querySelector('.added-model-provider-icon');
  const use = icon && icon.querySelector('use');
  let n = 0; try { n = use ? use.getNumberOfItems() : -1; } catch(e) { n = 'err:'+e.message; }
  return { title: document.title, cards, iconHtml: icon ? icon.innerHTML.slice(0,200) : null, nItems: n, href: use ? use.getAttribute('href') : null };
});
console.log(JSON.stringify(info, null, 1));
await page.screenshot({ path: 'providers-final3.png' });
await page.close();
await browser.close();
