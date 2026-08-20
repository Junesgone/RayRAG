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
    const info = await page.evaluate(() => {
      const cards = [...document.querySelectorAll('#dsList .ds-added-card')];
      return cards.map(c => ({
        type: c.dataset.type,
        rows: [...c.querySelectorAll('.ds-added-row')].map(r => ({
          name: r.querySelector('.ds-added-name') ? r.querySelector('.ds-added-name').textContent : '',
          buttons: [...r.querySelectorAll('button')].map(b => b.title || (b.textContent||'').trim()),
          legacy: ['Connect','Disconnect','List','Sync'].filter(t => (r.textContent||'').includes(t))
        }))
      }));
    });
    console.log(JSON.stringify(info));
    await page.screenshot({ path: '/tmp/rayrag_dsrows.png' });
    console.log('DONE');
  } catch (e) { console.error('ERR', e.message); } finally { await page.close(); }
})();
