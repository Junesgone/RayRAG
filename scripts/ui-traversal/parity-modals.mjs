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
async function openAndCheck(route, btnText, expected) {
  await page.goto('http://127.0.0.1:9390' + route + '?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(1000);
  const body = await page.evaluate((t) => { const els = Array.from(document.querySelectorAll('button,a')); const el = els.find(e => e.textContent.trim() === t) || els.find(e => e.textContent.includes(t)); if (el) { el.click(); return true; } return false; }, btnText);
  await page.waitForTimeout(800);
  const txt = await page.evaluate(() => document.body.innerText);
  const missing = expected.filter(e => !txt.includes(e));
  out.push({ route, clicked: body, missing });
  // close any modal
  await page.evaluate(() => { document.querySelectorAll('.show').forEach(m => m.classList.remove('show')); });
}
await openAndCheck('/datasets', 'Create dataset', ['Name', 'Embedding', 'Parse', 'Pipeline', 'Save', 'Close']);
await openAndCheck('/chats', 'Create chat', ['Name', 'Save']);
await openAndCheck('/searches', 'Create search', ['Name', 'Save']);
await openAndCheck('/agents', 'Create agent', ['Name', 'Agent flow', 'Ingestion pipeline', 'Save']);
await openAndCheck('/memories', 'Create memory', ['Name', 'raw', 'semantic', 'episodic', 'procedural', 'Embedding', 'LLM']);
await openAndCheck('/files', 'Add file', ['Upload file', 'Create empty file']);
console.log(JSON.stringify(out, null, 1));
await page.close();
await browser.close();
