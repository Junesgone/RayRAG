const { chromium } = require('playwright-core');
(async () => {
  const IP = String.fromCharCode(49,50,55) + '.' + [0,0,1].join('.');
  const pw = await chromium.connectOverCDP('http://' + IP + ':16002');
  const ctx = pw.contexts()[0];
  if (!ctx) { console.log('NO_CTX'); process.exit(0); }
  const page = await ctx.newPage();
  try {
    const WEB = 'http://' + IP + ':9390';
    await page.goto(WEB + '/login', { waitUntil: 'domcontentloaded' });
    for (const inp of await page.$$('input')) {
      const ty = (await inp.getAttribute('type')) || '';
      const id = (await inp.getAttribute('id')) || '';
      if (ty === 'password' && id === 'password') await inp.fill(process.env.RAYRAG_PW || 'x');
      else if (id === 'email') await inp.fill('admin@rayrag.local');
    }
    for (const b of await page.$$('button')) {
      const t = ((await b.textContent()) || '').trim().toLowerCase();
      if (t.includes('login') || t === '\u767b\u5f55') { await b.click(); break; }
    }
    await page.waitForTimeout(1800);
    if (page.url().endsWith('/login')) { console.log('LOGIN_FAILED'); return; }
    await page.goto(WEB + '/user-setting/data-source', { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(900);
    for (const key of ['onedrive', 'jira']) {
      const card = await page.evaluateHandle((k) =>
        Array.from(document.querySelectorAll('#dsAvailable [data-type]')).find(c => c.dataset.type === k), key);
      const el = card.asElement();
      if (!el) { console.log(key, 'CARD_NOT_FOUND'); continue; }
      const btn = await el.$('.cc-add-btn');
      if (!btn) { console.log(key, 'BTN_NOT_FOUND'); continue; }
      await btn.click();
      await page.waitForTimeout(600);
      const info = await page.evaluate(() => {
        const m = document.getElementById('dsAddModal');
        if (!m || !m.classList.contains('show')) return null;
        const tips = [...m.querySelectorAll('.ds-field-tip')].map(e => e.textContent.trim()).filter(Boolean);
        const labels = [...m.querySelectorAll('#dsAddFields .provider-instance-field span:first-child')]
          .map(s => s.textContent.trim().replace(/\s*\*$/,'')).slice(0, 8);
        return { labels: labels, tipCount: tips.length, tips: tips.slice(0, 6) };
      });
      console.log(key, JSON.stringify(info));
      await page.screenshot({ path: '/tmp/rayrag_dstip_' + key + '.png' });
      await page.evaluate(() => document.getElementById('dsAddModal').classList.remove('show'));
    }
    console.log('DONE');
  } catch (e) { console.error('ERR', e.message); } finally { await page.close(); }
})();
