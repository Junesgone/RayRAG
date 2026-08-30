
import { chromium } from 'playwright-core';
const out = [];
const log = (o) => out.push(o);
const b = await chromium.connectOverCDP('http://127.0.0.1:16002');
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const errs = [];
page.on('pageerror', e => errs.push('pageerror:' + String(e.message).slice(0,120)));
await page.goto('file:///tmp/rr/mp.html', { waitUntil: 'domcontentloaded', timeout: 45000 }).catch(e => log({ step:'goto_err', msg: String(e.message).slice(0,120) }));
await page.waitForTimeout(1500);
const init = await page.evaluate(() => ({
  chips: Array.from(document.querySelectorAll('.provider-chip')).map(c => (c.textContent||'').trim().replace(/\s+/g,' ')),
  cards: document.querySelectorAll('[data-testid=available-model-card]').length,
  badges: [...new Set(Array.from(document.querySelectorAll('.available-provider-tags .badge')).map(x=>(x.textContent||'').trim()))],
}));
log({ step: 'deployed_html_init', ...init });
const filters = await page.evaluate(() => Array.from(document.querySelectorAll('.provider-chip')).map(c => c.getAttribute('data-filter') || ''));
for (const f of filters) {
  const res = await page.evaluate((flt) => {
    const chip = Array.from(document.querySelectorAll('.provider-chip')).find(c => (c.getAttribute('data-filter') || '') === flt);
    if (!chip) return { error: 'chip not found', flt };
    chip.click();
    const shown = Array.from(document.querySelectorAll('[data-testid=available-model-card]')).filter(c => c.offsetParent !== null);
    return {
      clicked_label: (chip.textContent||'').trim().replace(/\s+/g,' '),
      pill_on: Array.from(document.querySelectorAll('.provider-chip.on')).map(c=>(c.textContent||'').trim().replace(/\s+/g,' ')),
      visible: shown.length,
      first_names: shown.slice(0,3).map(c => c.dataset.name),
      first_badges: shown.slice(0,3).map(c => Array.from(c.querySelectorAll('.available-provider-tags .badge')).map(x=>(x.textContent||'').trim()).join('+')),
    };
  }, f);
  log({ step: 'chip_click', filter: f, ...res });
}
await page.screenshot({ path: '/home/junesgone/.openclaw/workspace/RayRAG/docs/ui-traversal/rayrag-audit/model-chips-deployed-20260830.png', fullPage: false });
log({ step: 'pageerrors', count: errs.length, sample: errs.slice(0,3) });
await page.close(); await b.close();
process.stdout.write(JSON.stringify(out));

