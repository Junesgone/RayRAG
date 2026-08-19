import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2500);
const title = await page.title();
const icons = await page.evaluate(() => ({
  useRefs: Array.from(document.querySelectorAll('use')).map(u => u.getAttribute('href')).slice(0, 8),
  imgs: Array.from(document.querySelectorAll('img.provider-brand-icon')).map(i => i.src).slice(0, 5),
  renderedUse: Array.from(document.querySelectorAll('use')).filter(u => { const d = u.ownerSVGElement && u.ownerSVGElement.querySelector('path'); return !!d; }).length
}));
console.log(JSON.stringify({ title, ...icons }, null, 1));
await page.close();
await browser.close();
