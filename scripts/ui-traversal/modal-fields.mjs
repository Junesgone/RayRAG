import { chromium } from 'playwright-core';
const browser = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.goto('http://127.0.0.1:9390/login');
await page.evaluate(async () => {
  const r = await fetch('/api/v1/auth/login', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({email:'admin@rayrag.local', password:'rayrag-smoke-admin'})});
  const j = await r.json();
  localStorage.setItem('rayrag_token', j.data.access_token);
  document.cookie = 'rayrag_token='+j.data.access_token+';path=/;max-age=86400';
});
const out = [];
async function inspect(route, btnText) {
  await page.goto('http://127.0.0.1:9390' + route + '?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(800);
  const clicked = await page.evaluate((t) => { const els = Array.from(document.querySelectorAll('button')); const el = els.find(e => e.textContent.trim().includes(t)); if (el) { el.click(); return true; } return false; }, btnText);
  await page.waitForTimeout(500);
  const fields = await page.evaluate(() => {
    const m = document.querySelector('.modal-overlay.show, .modal.show, [class*=modal][style*=block]') || document.querySelector('.modal');
    const scope = m ? (m.closest('.modal-overlay') || m) : document.body;
    return Array.from(scope.querySelectorAll('input,select,textarea,button,label'))
      .map(el => (el.tagName === 'INPUT' ? (el.type||'').toUpperCase() : el.tagName) + ':' + (el.placeholder || el.value || el.textContent || '').trim().slice(0,24))
      .filter(s => s.length > 6).slice(0, 24);
  });
  out.push({ route, clicked, fields });
  await page.evaluate(() => { document.querySelectorAll('.modal-overlay.show,.modal-overlay.show').forEach(m=>m.classList.remove('show')); });
}
await inspect('/kbs', 'New KB');
await inspect('/agents', 'New Agent');
await inspect('/memories', 'Create Memory');
await inspect('/chats', 'New Chat');
await inspect('/searches', 'New Search');
console.log(JSON.stringify(out, null, 1));
await page.close();
await browser.close();
