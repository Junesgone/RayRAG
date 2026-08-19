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
await page.goto('http://127.0.0.1:9390/user-setting/model?cb=' + Date.now(), { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
const info = await page.evaluate(() => {
  function inkRatio(svg) {
    try {
      const xml = new XMLSerializer().serializeToString(svg);
      const blob = new Blob([xml], {type:'image/svg+xml;charset=utf-8'});
      const url = URL.createObjectURL(blob);
      const img = new Image();
      img.src = url;
      // synchronous check won't work; do it via document height/width instead
      return { w: svg.getBoundingClientRect().width, h: svg.getBoundingClientRect().height };
    } catch(e) { return String(e); }
  }
  const svgs = Array.from(document.querySelectorAll('.provider-brand-icon')).slice(0, 12);
  const out = svgs.map(s => { const r = s.getBoundingClientRect(); return { tag: s.tagName.toLowerCase(), href: s.querySelector('use') ? s.querySelector('use').getAttribute('href') : null, w: r.width, h: r.height }; });
  return out;
});
console.log(JSON.stringify(info, null, 1));
await page.screenshot({ path: 'providers-final5.png' });
await page.close();
await browser.close();
