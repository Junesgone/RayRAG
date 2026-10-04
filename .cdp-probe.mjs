import { chromium } from 'playwright-core';
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = ctx.pages().find(p => p.url().includes('9390')) || (await ctx.newPage());
const errors = [];
page.on('pageerror', e => errors.push(String(e).slice(0, 90)));
const tok = (await (await fetch('http://127.0.0.1:9390/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'admin@rayrag.local', password: '<redacted>' }) })).json()).data.access_token;
await page.goto('http://127.0.0.1:9390/login', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(() => {});
await page.evaluate(t => localStorage.setItem('rayrag_token', t), tok);
const KB = '0bd5976a-6b50-49ba-8a07-07b4952a51cc';
for (const lang of ['zh-CN', 'en-US']) {
  await page.context().addCookies([{ name: 'lang', value: lang, url: 'http://127.0.0.1:9390' }]).catch(() => {});
  await page.goto(`http://127.0.0.1:9390/dataset/configuration/${KB}`, { waitUntil: 'networkidle', timeout: 45000 }).catch(() => {});
  await page.waitForTimeout(7000);
  const before = await page.evaluate(() => {
    const m = document.getElementById('linkDataSourceModal');
    return { path: location.pathname, exists: !!m, shown: !!m && m.offsetParent !== null };
  });
  await page.locator('#cfgLinkDataSource').click({ timeout: 15000 }).catch(e => console.log('click error', String(e).slice(0, 60)));
  await page.waitForTimeout(1500);
  const opened = await page.evaluate(() => {
    const m = document.getElementById('linkDataSourceModal');
    const sel = document.getElementById('linkDataSourceKind');
    return {
      stillOnDatasetPage: location.pathname.startsWith('/dataset/'),
      shown: !!m && m.offsetParent !== null,
      title: m ? (m.querySelector('h3') || {}).textContent : null,
      buttons: m ? [...m.querySelectorAll('button')].map(x => x.textContent.trim()) : [],
      options: sel ? sel.options.length : 0,
      firstOption: sel ? sel.options[0].textContent : null,
    };
  });
  console.log(lang, 'before:', JSON.stringify(before), 'after click:', JSON.stringify(opened));
  await page.locator("[data-testid='dataset-link-data-source-cancel']").click({ timeout: 10000 }).catch(() => {});
  await page.waitForTimeout(1200);
  const closed = await page.evaluate(() => {
    const m = document.getElementById('linkDataSourceModal');
    return { shown: !!m && m.offsetParent !== null, path: location.pathname };
  });
  console.log(lang, 'after Cancel:', JSON.stringify(closed));
}
console.log('pageerrors:', JSON.stringify(errors));
await b.close();
