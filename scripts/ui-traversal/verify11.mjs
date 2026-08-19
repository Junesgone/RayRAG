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
const info = await page.evaluate(async () => {
  const list = document.getElementById('addedModelList');
  let api = null;
  try { const r = await fetch('/api/v1/tenant/models', {headers:{'Authorization':'Bearer '+localStorage.getItem('rayrag_token')}}); api = { status: r.status, body: (await r.text()).slice(0,300) }; } catch(e) { api = String(e); }
  return { listHtml: list ? list.innerHTML.slice(0,300) : null, api };
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
