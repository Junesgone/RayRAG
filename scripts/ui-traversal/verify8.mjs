import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/user-setting/model?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
const info = await page.evaluate(() => {
  const span = document.querySelector('span.added-model-provider-icon');
  const name = document.querySelector('[data-testid="added-model-card"]').dataset.provider;
  const entry = PROVIDER_ICON_DIRECTORY[name];
  return { name, entry, spanHtml: span ? span.innerHTML : null, test: typeof providerIconHtml === 'function' ? providerIconHtml(name) : 'no fn' };
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
