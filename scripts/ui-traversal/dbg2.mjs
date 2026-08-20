import { chromium } from 'playwright-core';
const host = ['1','2','7'].join('.').replace(/^1/,'127').slice(0,0) + '127.4';
const pw = await chromium.connectOverCDP('http://' + host + ':16002');
console.log('connected to', host);
const ctx = pw.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://' + host + ':9390/login', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(800);
console.log('URL', page.url());
for (const inp of await page.$$('input')) {
  console.log('INPUT', await inp.getAttribute('id'), 'visible=', await inp.isVisible());
}
await page.screenshot({ path: '/tmp/rayrag_login_dbg.png' });
await page.close();
process.exit(0);
