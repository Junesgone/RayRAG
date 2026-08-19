import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2500);
const info = await page.evaluate(() => {
  const uses = Array.from(document.querySelectorAll('use'));
  const out = uses.slice(0, 6).map(u => { let n = u; let ctx2=''; for (let i=0;i<6 && n;i++){ n = n.parentElement; if (n) ctx2 += (n.tagName.toLowerCase()+(n.id?('#'+n.id):'')+(n.className&&typeof n.className==='string'?('.'+n.className.split(' ')[0]):''))+' > '; }
    return { href: u.getAttribute('href'), chain: ctx2 } });
  return out;
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
