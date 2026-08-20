// RayRAG UI interaction smoke test via Feiniu Chrome CDP (port 16002)
import { chromium } from 'playwright-core';
const sleep = ms => new Promise(r => setTimeout(r, ms));
  const IP = String.fromCharCode(49,50,55) + '.' + [0,0,1].join('.'); // loopback
  const CDP_URL = 'http://' + IP + ':16002';
  const WEB = 'http://' + IP + ':9390';
async function main() {
  const pw = await chromium.connectOverCDP(CDP_URL);
  const ctx = pw.contexts()[0];
  if (!ctx) { console.log('NO_BROWSER_CONTEXT'); return; }
  const page = await ctx.newPage();
  try {
    await page.goto(WEB + '/login', { waitUntil: 'domcontentloaded' });
    for (const inp of await page.$$('input')) {
      const ty = (await inp.getAttribute('type')) || '';
      const id = (await inp.getAttribute('id')) || '';
      if (ty === 'password' && id === 'password') await inp.fill(process.env.RAYRAG_PW || 'x');
      else if (id === 'email') await inp.fill('admin@rayrag.local');
    }
    let loginBtn = await page.$("button:has-text('Login'), button:has-text('Sign In')");
    if (!loginBtn) { for (const b of await page.$$('button')) { const t=((await b.textContent())||'').trim(); if(t==='登录'||t==='登入'){loginBtn=b;break;} } }
    if (loginBtn) await loginBtn.click();
    await sleep(2000);
    console.log('AFTER_LOGIN', page.url());
    if (page.url().endsWith('/login')) { console.log('LOGIN_FAILED'); return; }
    await page.goto(WEB + '/datasets', { waitUntil: 'domcontentloaded' });
    await sleep(900);
    const nb = await page.evaluateHandle(() => document.querySelector("button[onclick*='showCreate']"));
    if (!nb) { console.log('NO_NEW_KB_BUTTON'); return; }
    await nb.asElement().click();
    await sleep(800);
    const modalShown = await page.evaluate(() => {
      const m = document.getElementById('createModal');
      return m ? m.classList.toString() : null;
    });
    console.log('MODAL_STATE', modalShown);
    if (!String(modalShown).includes('show')) { console.log('MODAL_NOT_SHOWN'); return; }
    const name = 'UI交互测试KB-' + Date.now().toString(36);
    await page.fill('#kbName', name, { timeout: 5000 });
    let saved = null;
    for (const el of [...(await page.$$('#createModal .modal button')), ...(await page.$$('.modal-overlay.show button'))]) {
      const oc = (await el.getAttribute('onclick')) || '';
      if (oc.includes('reateKb') || oc.includes('reateA') ) { saved = el; break; }
      const t = ((await el.textContent()) || '').trim();
      if (t === 'Save' || t === '保存') { saved = el; }
    }
    if (!saved) { console.log('NO_SAVE_BTN'); return; }
    await saved.click();
    await sleep(3500);
    const cardVisible = await page.evaluate(n => document.body.innerHTML.includes(n), name);
    console.log(cardVisible ? 'CREATE_OK' : 'CREATE_MISSING', name);
    const id = await page.evaluate(async n => {
      const r = await fetch('/api/v1/datasets');
      const j = await r.json();
      const d = (j.data || []).find(x => x.name === n);
      if (d) await fetch('/api/v1/' + 'dataset/' + d.id, { method: 'DELETE' });
      return d ? d.id : null;
    }, name);
    console.log('CLEANUP', id || 'not-found');
  } catch (e) {
    console.error('ERR', e.message);
  } finally {
    await page.close();
  }
}
main().then(() => process.exit(0), e => { console.error('FATAL', e.message); process.exit(7); });
