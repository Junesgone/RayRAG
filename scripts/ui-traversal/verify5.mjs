import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/user-setting/model', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(2500);
const info = await page.evaluate(() => {
  const cards = Array.from(document.querySelectorAll('[data-testid="added-model-card"]'));
  const out = cards.slice(0,6).map(c => {
    const name = c.dataset.provider;
    const entry = ADDED_PROVIDER_DIRECTORY.find(p => p.id === name);
    const icon = PROVIDER_ICON_DIRECTORY[name];
    return { cardName: name, dirEntry: entry, iconEntry: icon };
  });
  return { added: ADDED_PROVIDER_DIRECTORY, out };
});
console.log(JSON.stringify(info, null, 1));
await page.close();
await browser.close();
