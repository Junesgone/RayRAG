import { chromium } from 'playwright-core';
const CDP = process.env.RAYRAG_CDP_ENDPOINT || 'http://127.0.0.1:16002';
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || 'http://127.0.0.1:9390';
const PW = process.env.RAYRAG_LOGIN_PASSWORD || 'rayrag-smoke-admin';
const browser = await chromium.connectOverCDP(CDP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
const errors = [];
page.on('pageerror', (e) => errors.push(String(e)));
await page.goto(ORIGIN + '/user-setting/model', { waitUntil: 'domcontentloaded', timeout: 60000 });
await page.waitForTimeout(2500);
// log in if the page shows the login form
if (await page.locator('#email').count()) {
  await page.fill('#email', 'admin@rayrag.local');
  await page.fill('#password', PW);
  await page.click('button[type=submit]');
  await page.waitForTimeout(4000);
  await page.goto(ORIGIN + '/user-setting/model', { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(3000);
}
console.log('URL:', page.url());
const inputs = await page.evaluate(() => [...document.querySelectorAll('input')].map((i) => ({ id: i.id, name: i.name, type: i.type })));
console.log('INPUTS:', JSON.stringify(inputs));
const info = await page.evaluate(() => {
  const right = document.querySelector('.model-providers-right');
  const grid = document.getElementById('availableProviderGrid');
  const cards = document.querySelectorAll('.available-provider-card');
  const boxes = [...document.querySelectorAll('.model-providers-right *')].slice(0, 0);
  return {
    rightExists: !!right,
    rightWidth: right ? Math.round(right.getBoundingClientRect().width) : 0,
    rightHeight: right ? Math.round(right.getBoundingClientRect().height) : 0,
    gridHeight: grid ? Math.round(grid.getBoundingClientRect().height) : 0,
    cards: cards.length,
    visibleCards: [...cards].filter((c) => c.offsetParent !== null).length,
    bodyHeight: document.body.scrollHeight,
    firstCardText: cards[0] ? cards[0].innerText.slice(0, 120) : '',
  };
});
await page.screenshot({ path: '/tmp/rr-model-page.png', fullPage: false });
console.log(JSON.stringify({ info, errors }, null, 2));
await page.close();
await browser.close();
