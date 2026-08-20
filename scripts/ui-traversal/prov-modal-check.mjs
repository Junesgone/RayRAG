// CDP check: open provider instance dialog for XunFei Spark & BaiduYiyan on RayRAG model page
import { chromium } from 'playwright-core';
const IP = String.fromCharCode(49,50,55) + '.' + [0,0,1].join('.');
async function main() {
  const pw = await chromium.connectOverCDP('http://' + IP + ':16002');
  const ctx = pw.contexts()[0];
  if (!ctx) { console.log('NO_CTX'); return; }
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
      if (t.includes('login') || t === '登录') { await b.click(); break; }
    }
    await page.waitForTimeout(1800);
    if (page.url().endsWith('/login')) { console.log('LOGIN_FAILED'); return; }
    await page.goto(WEB + '/user-setting/model', { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(900);
    for (const target of ['XunFei Spark', 'BaiduYiyan']) {
      const card = await page.evaluateHandle((name) =>
        Array.from(document.querySelectorAll('[data-testid="available-model-card"]'))
          .find(c => c.dataset.provider === name), target);
      const el = card.asElement();
      if (!el) { console.log(target, 'CARD_NOT_FOUND'); continue; }
      const btn = await el.$("button[onclick*='showProviderInstanceModal']");
      if (!btn) { console.log(target, 'BTN_NOT_FOUND'); continue; }
      await btn.click();
      await page.waitForTimeout(700);
      const info = await page.evaluate(() => {
        const m = document.getElementById('providerInstanceModal');
        if (!m || !m.classList.contains('show')) return null;
        const labels = [...m.querySelectorAll('.provider-instance-field span')].map(s => s.textContent.trim());
        const ids = [...m.querySelectorAll('#piExtraFields input,#piExtraFields select')].map(e => e.id);
        const types = [...document.querySelectorAll('#piTypeFields input:checked')].map(i => i.value);
        return { labels, ids, types };
      });
      console.log(target, JSON.stringify(info));
      await page.screenshot({ path: '/tmp/rayrag_prov_' + target.replace(/\s+/g,'_') + '.png' });
      await page.evaluate(() => document.getElementById('providerInstanceModal').classList.remove('show'));
    }
    console.log('DONE');
  } catch (e) { console.error('ERR', e.message); } finally { await page.close(); }
}
main().then(() => process.exit(0), e => { console.error('FATAL', e.message); process.exit(7); });
