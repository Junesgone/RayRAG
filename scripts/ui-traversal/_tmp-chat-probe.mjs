import { chromium } from 'playwright-core';
const base = 'http://127.0.0.1:9390';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0] || await browser.newContext();
const page = await ctx.newPage();
const errors = [];
page.on('pageerror', e => errors.push(String(e).slice(0, 200)));
await page.goto(base + '/login', { waitUntil: 'domcontentloaded', timeout: 60000 });
// Sign in with the deployed admin account so the page renders like a real user sees it.
if (page.url().includes('/login')) {
  await page.fill('input[type=email], input[name=email]', 'admin@rayrag.local').catch(() => {});
  await page.fill('input[type=password]', 'rayrag-smoke-admin').catch(() => {});
  await page.click('button[type=submit]').catch(() => {});
  await page.waitForTimeout(3000);
}
await page.goto(base + '/chat', { waitUntil: 'networkidle', timeout: 60000 });
await page.waitForTimeout(2000);
const info = await page.evaluate(() => {
  const text = document.body.innerText || '';
  const bad = [];
  const re = /[\uFFFD]|Ã.|â€.|ï¿½/g;
  let m;
  while ((m = re.exec(text)) && bad.length < 12) {
    bad.push(text.slice(Math.max(0, m.index - 40), m.index + 40));
  }
  return {
    url: location.href,
    len: text.length,
    replacement: (text.match(/\uFFFD/g) || []).length,
    samples: bad,
    head: text.slice(0, 400),
    chinese: (text.match(/[\u4e00-\u9fff]+/g) || []).slice(0, 20),
  };
});
console.log(JSON.stringify(info, null, 1));
console.log('console errors:', JSON.stringify(errors));
await page.close();
await browser.close();
