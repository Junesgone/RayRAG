import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2000);
const info = await page.evaluate(() => {
  const keys = Object.keys(PROVIDER_ICON_DIRECTORY).filter(k => /Qwen|mxbai|PaddleOCR|Local/.test(k));
  const out = keys.map(k => [k, PROVIDER_ICON_DIRECTORY[k]]);
  const html = document.querySelector('span.added-model-provider-icon').innerHTML;
  return { out, html };
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
