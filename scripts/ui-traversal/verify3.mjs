import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/login');
const tok = await page.evaluate(async () => {
  const r = await fetch('/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password:'rayrag-smoke-admin'})});
  const j = await r.json();
  localStorage.setItem('rayrag_token', j.data.access_token);
  document.cookie = 'rayrag_token='+j.data.access_token+';path=/;max-age=86400';
  return j.data.access_token.slice(0,8);
});
console.log('tok', tok);
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2500);
const info = await page.evaluate(() => {
  const uses = Array.from(document.querySelectorAll('use'));
  const rendered = uses.filter(u => u.ownerSVGElement && u.ownerSVGElement.querySelector('path')).length;
  let dir = null; try { dir = { 'OpenAI': PROVIDER_ICON_DIRECTORY['OpenAI'], 'Gemini': PROVIDER_ICON_DIRECTORY['Gemini'] }; } catch(e) { dir = String(e); }
  return { title: document.title, useCount: uses.length, rendered, sample: uses.slice(0,3).map(u=>u.getAttribute('href')), dir };
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
