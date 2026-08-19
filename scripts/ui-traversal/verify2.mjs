import { chromium } from 'playwright-core';
import https from 'node:https';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
// login via API
const res = await fetch('http://127.0.0.1:9390/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password: process.env.RAYRAG_ADMIN_PASSWORD})});
const j = await res.json();
const tok = j.data.access_token;
await page.goto('http://127.0.0.1:9390/login');
await page.evaluate(t => { localStorage.setItem('rayrag_token', t); document.cookie = 'rayrag_token='+t+';path=/;max-age=86400'; }, tok);
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2000);
const info = await page.evaluate(() => {
  const uses = Array.from(document.querySelectorAll('use'));
  const rendered = uses.filter(u => u.ownerSVGElement && u.ownerSVGElement.querySelector('path')).length;
  return { title: document.title, useCount: uses.length, rendered, sample: uses.slice(0,3).map(u=>u.getAttribute('href')), imgs: Array.from(document.querySelectorAll('img.provider-brand-icon')).slice(0,4).map(i=>i.getAttribute('src')) };
});
console.log(JSON.stringify(info, null, 1));
await page.screenshot({ path: 'providers-after-cleanup.png' });
await page.close();
await browser.close();
