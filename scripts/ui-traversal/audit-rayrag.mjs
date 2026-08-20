// RayRAG SSR UI click-by-click audit via fnOS Chrome CDP.
// Usage: export RAYRAG_ADMIN_EMAIL / RAYRAG__ADMIN_PASSWORD, then:
//   node audit-rayrag.mjs            (all routes)
//   ROUTE_FILTER=/user-setting/model,node audit audit-rayrag.mjs
import { chromium } from 'playwright-core';
import { writeFileSync, mkdirSync, readFileSync, existsSync } from 'node:fs';
import path from 'node:path';

import fs from 'node:fs';
globalThis.__log=(m)=>{process.stdout.write(new Date().toISOString()+' '+m+String.fromCharCode(10));};
setTimeout(()=>{__log('WATCHDOG exit');process.exit(9)},25*60*1000).unref?.();
const CDPU = process.env.CDP_URL || 'http://127.0.0.1:16002';
const RBASE = process.env.RAYRAG_UI || 'http://127.0.0.1:9390';
const OUT_DIR = path.resolve(process.env.OUT_DIR || '/home/junesgone/.openclaw/workspace/RayRAG/docs/ui-traversal/rayrag-audit');
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

// Self-contained in-page overlay snapshot (must not reference outer scope).
function overlayFn() {
  const out = [];
  for (const el of document.querySelectorAll('[role="dialog"], .modal-overlay.show, [class*="popover"]')) {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 300);
      if (t) out.push(t);
    }
  }
  return out;
}

const ROUTES = [
  '/dashboard',
  '/kbs',
  '/chats',
  '/searches',
  '/agents',
  '/agent-templates',
  '/memories',
  '/files',
  '/files/skills',
  '/user-setting',
  '/user-setting/profile',
  '/user-setting/model',
  '/user-setting/mcp',
  '/user-setting/team',
  '/user-setting/api',
  '/user-setting/data-source',
  '/user-setting/chat-channel',
  '/admin',
];

const DESTRUCTIVE = /delete|remove|discard|confirm|reset|revoke|logout|log out|stop|purge|drop|cancel|删除|移除|清空|确认|撤销|登出|停止|销毁/i;

const browser = await chromium.connectOverCDP(CDPU);
const ctx = browser.contexts()[0] || (await browser.newContext());
__log('creating audit page');
let page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 900 }).catch(() => {});
page.on("dialog",d=>{try{d.dismiss()}catch(e){}});

// ---- login --------------------------------------------------------------
{
  const email = process.env.RAYRAG_ADMIN_EMAIL || 'admin@rayrag.local';
  const password = process.env.AUDIT_PW || '';
  if (!password) { console.error('missing AUDIT_PW'); process.exit(2); }
  const r = await fetch(RBASE + '/api/v1/auth/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email, password }),
  });
  const j = await r.json().catch(() => ({}));
  if (j.code !== 0) { console.error('login failed:', j.message || String(r.status)); process.exit(3); }
  await page.goto(RBASE + '/login', { waitUntil: 'domcontentloaded', timeout: 60000 });
  await page.evaluate((t) => localStorage.setItem('rayrag_token', t), j.data.access_token);
}

const FILTER = (process.env.ROUTE_FILTER || '').split(',').filter(Boolean);
let routes = ROUTES;
if (FILTER.length) routes = FILTER;

mkdirSync(OUT_DIR, { recursive: true });
const reportPath = path.join(OUT_DIR, 'rayrag-ui-map.json');
const prevReport = existsSync(reportPath) ? JSON.parse(readFileSync(reportPath, 'utf8')) : null;
const report = { base: RBASE, generated_at: new Date().toISOString(), routes: [] };
if (prevReport && FILTER.length) {
  report.routes = (prevReport.routes || []).filter(r => !FILTER.includes(r.route));
}

async function ensurePage() {
  if (!page.isClosed()) return page;
  __log('page closed, recreating');
  try { page = await ctx.newPage(); } catch(e){ throw e; }
  await page.setViewportSize({ width: 1600, height: 900 }).catch(()=>{});
  return page;
}

async function safeGoto(route){
  for(let i=0;i<4;i++){
    try{ await ensurePage();
      await page.goto(RBASE+route,{waitUntil:'domcontentloaded',timeout:60000});
      return true;
    }catch(e){ __log('goto fail '+route+' #'+i+': '+String(e).slice(0,120)); }
  }
  return false;
}

for (const route of routes) {
  const rec = { route, ok: false, elements: [], note: '' };
  try {
    if (!(await safeGoto(route))) {
      rec.note = 'goto failed after retries';
      report.routes.push(rec);
      console.log(`[${report.routes.length}/${routes.length}] ${route} FAILED`);
      writeFileSync(reportPath, JSON.stringify(report, null, 2));
      continue;
    }
    await sleep(2500);
    rec.ok = true;
    rec.title = await page.title().catch(() => '');
    rec.body_text_head = await page.evaluate(() => (document.body.innerText || '').slice(0, 800)).catch(() => '');
    const shotPath = path.join(OUT_DIR, `shot-${route.replace(/\//g, '_').replace(/^_/, '')}.png`);
    await page.screenshot({ path: shotPath }).catch(() => {});
    rec.shot = path.basename(shotPath);

    const els = await page.evaluate(() => {
      const sels = ['button', 'a[href]', '[role="tab"]', '.user-setting-nav-item', 'summary'];
      const seen = new Set();
      const out = [];
      for (const s of sels) {
        for (const el of document.querySelectorAll(s)) {
          if (seen.has(el)) continue;
          const r = el.getBoundingClientRect();
          if (r.width < 1 || r.height < 1) continue;
          if (el.closest('.modal-overlay.show')) continue;
          const text = (el.innerText || el.textContent || el.getAttribute('aria-label') || '').trim().replace(/\s+/g, ' ').slice(0, 120);
          if (!text && !/button|a|summary/.test(String(el.tagName).toLowerCase())) continue;
          seen.add(el);
          out.push({ kind: s, text: text || '(icon)', role: String(el.tagName).toLowerCase() });
        }
      }
      return out;
    }).catch(() => []);

    const uniqMap = new Map();
    for (const e of els) {
      const k = e.text + '|' + e.role;
      if (!uniqMap.has(k)) uniqMap.set(k, e);
    }
    rec.total_interactive = uniqMap.size;

    const CANDIDATES = [...uniqMap.values()].filter(e => !DESTRUCTIVE.test(e.text) && e.text!==String.fromCharCode(20013) && e.text!=='EN').slice(0, 40);
    for (const e of CANDIDATES) {
      const entry = { text: e.text, role: e.role, result: null };
      try {
        const beforeUrl = page.url();
        const overlaysBefore = await page.evaluate(overlayFn).catch(() => []);
        const clicked = await page.evaluate(({ kind, text }) => {
          let candidates = [];
          try { candidates = Array.from(document.querySelectorAll(kind)); } catch { return false; }
          const el = candidates.find(x => ((x.innerText || x.textContent || x.getAttribute('aria-label') || '').trim().replace(/\s+/g, ' ')).slice(0, 120) === text);
          if (!el) return false;
          el.scrollIntoView({ block: 'center' });
          try { el.click(); } catch { return false; }
          return true;
        }, e).catch(() => false);
        if (!clicked) { entry.result = 'skipped:not-found'; rec.elements.push(entry); continue; }
        await sleep(800);
        const overlaysAfter = await page.evaluate(overlayFn).catch(() => []);
        const newOverlays = overlaysAfter.filter(o => !overlaysBefore.includes(o));
        const urlNow = page.url();
        const navChanged = urlNow !== beforeUrl;
        entry.result = [];
        if (newOverlays.length) entry.result.push('popup: ' + newOverlays.join(' | ').slice(0, 300));
        if (navChanged) entry.result.push('nav -> ' + urlNow.replace(RBASE, ''));
        if (!entry.result.length) {
          const after = await page.evaluate(() => (document.body.innerText || '').slice(0, 800)).catch(() => '');
          entry.result.push(after !== rec.body_text_head ? 'page-text-changed' : 'no visible popup or nav');
        }
      } catch (err) { entry.result = 'error: ' + String(err).slice(0, 200); }
      rec.elements.push(entry);

      // restore route before the next probe
      try {
        let guard = 0;
        while (!page.url().includes(route) && guard++ < 4) {
          await page.keyboard.press('Escape').catch(() => {});
          const closed = await page.evaluate(() => {
            for (const ov of document.querySelectorAll('.modal-overlay.show')) {
              ov.classList.remove('show');
              return true;
            }
            return false;
          }).catch(() => false);
          await sleep(300);
          if (!closed || !page.url().includes(route)) {
            await page.goto(RBASE + route, { waitUntil: 'domcontentloaded', timeout: 60000 }).catch(() => {});
            await sleep(2000);
          }
        }
      } catch {}
    }
  } catch (e) { rec.note = String(e).slice(0, 300); }
  report.routes.push(rec);
  console.log(`[${report.routes.length}/${routes.length}] ${route} ok=${rec.ok} total=${rec.total_interactive || 0} note=${rec.note || ''}`);
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
}

writeFileSync(reportPath, JSON.stringify({ ...report, merged_prev_routes: prevReport && FILTER.length ? (prevReport.routes || []).length : 0 }, null, 2));
console.log('done ->', reportPath);
process.exit(0);
