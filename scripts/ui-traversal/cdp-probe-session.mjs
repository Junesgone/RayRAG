
import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
console.log('open_pages:', ctx.pages().length, ctx.pages().map(p=>p.url()).join(' | ').slice(0,300));
const page = await ctx.newPage();
for (const [name,url] of [['rayrag','http://127.0.0.1:9390/user-setting/model'],['ragflow','http://127.0.0.1:11526/']]) {
  try {
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 40000 });
    await page.waitForTimeout(2500);
    const info = await page.evaluate(() => ({
      path: location.pathname,
      login: !!document.querySelector('input[type=password]'),
      buttons: Array.from(document.querySelectorAll('button')).slice(0,14).map(b=>(b.textContent||'').trim()).filter(Boolean),
    }));
    console.log(name, JSON.stringify(info).slice(0,800));
  } catch (e) { console.log(name, 'ERR', String(e.message).slice(0,150)); }
}
await page.close(); await b.close();

